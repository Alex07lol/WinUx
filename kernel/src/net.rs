//! Network stack glue (Phase 6, stage 5): one e1000 NIC, static IPv4
//! configuration, and ARP/ICMP/UDP built on the host-tested `os_core` codecs.
//!
//! The driver is polled and synchronous. Every operation that waits for a
//! reply runs a bounded poll loop that takes the network lock per iteration,
//! so timer and serial interrupts keep flowing between iterations and PIT
//! deadlines stay meaningful.

use alloc::string::String;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use os_core::net::arp::{self, Cache};
use os_core::net::eth::{self, MacAddr};
use os_core::net::icmp;
use os_core::net::ipv4::{self, Addr};
use os_core::net::udp;

use crate::arch::x86_64::{pit, read_tsc};
use crate::e1000::{E1000, Loopback};
use crate::sync::IrqSpinLock;

/// Static configuration matching QEMU's user-mode network (and VirtualBox's
/// default NAT): guest 10.0.2.15/24, gateway 10.0.2.2.
pub const IP_ADDRESS: Addr = Addr([10, 0, 2, 15]);
pub const SUBNET_PREFIX: u8 = 24;
pub const GATEWAY: Addr = Addr([10, 0, 2, 2]);

const ECHO_IDENTIFIER: u16 = 0x4F53;
const MAX_FRAME_BYTES: usize = 1536;
const ARP_ENTRIES: usize = 8;
const RX_DRAIN_PER_POLL: usize = 8;

const ARP_ATTEMPTS: u8 = 3;
const ARP_ATTEMPT_TICKS: u64 = 100; // 1 s at the 100 Hz PIT
const ECHO_ATTEMPTS: u8 = 3;
const ECHO_ATTEMPT_TICKS: u64 = 100;
const UDPECHO_TIMEOUT_TICKS: u64 = 1000; // 10 s
const UDPECHO_MAX_PAYLOAD: usize = 1024;

struct IcmpReply {
    source: Addr,
    ttl: u8,
    sequence: u16,
}

struct UdpechoJob {
    port: u16,
    done: bool,
    echoed: usize,
    peer: (Addr, u16),
}

struct NetState {
    nic: E1000,
    cache: Cache<ARP_ENTRIES>,
    last_echo: Option<IcmpReply>,
    udpecho: Option<UdpechoJob>,
}

static NET: IrqSpinLock<Option<NetState>> = IrqSpinLock::new(None);
static SELFTEST_PASSED: AtomicBool = AtomicBool::new(false);
static TSC_FREQUENCY: AtomicU64 = AtomicU64::new(0);

/// Probe PCI for the NIC and bring it up. Logs the pinned `net:` lines.
/// Start the NIC that the driver model has already matched and claimed.
/// `Ok` means the MAC is read, the loopback selftest ran and the polled
/// network state is installed; `Err` carries a stable reason for the driver
/// binding's `Failed` state. A machine without a NIC never calls this.
pub fn init<F>(
    device: &crate::pci::PciDevice,
    hhdm_offset: u64,
    tsc_frequency: Option<u64>,
    claim: F,
) -> Result<(), &'static str>
where
    F: FnOnce([(u64, u32); 4]) -> Result<(), &'static str>,
{
    TSC_FREQUENCY.store(tsc_frequency.unwrap_or(0), Ordering::SeqCst);

    crate::kprintln!(
        "net: e1000 at {:02x}:{:02x}.{} bar0 {:#x} size {} KiB",
        device.bus,
        device.device,
        device.function,
        device.bar0.base,
        device.bar0_size / 1024
    );
    crate::pci::enable_device(device.bus, device.device, device.function);

    let mut nic = match E1000::init(device, hhdm_offset, claim) {
        Ok(nic) => nic,
        Err(reason) => {
            crate::kprintln!("net: e1000 init failed ({})", reason);
            return Err(reason);
        }
    };
    crate::kprintln!("net: mac {}", nic.mac());
    crate::kprintln!("net: ip {}/{} gw {}", IP_ADDRESS, SUBNET_PREFIX, GATEWAY);

    let outcome = nic.loopback_selftest();
    crate::kprintln!(
        "net: loopback selftest: {}",
        match outcome {
            Loopback::Passed => "passed",
            Loopback::Unsupported => "unsupported",
            Loopback::UnexpectedFrame => "unexpected frame",
        }
    );

    *NET.lock() = Some(NetState {
        nic,
        cache: Cache::new(),
        last_echo: None,
        udpecho: None,
    });
    Ok(())
}

/// Stop the polled NIC and release its ring frames. The network state is
/// removed first, so even a teardown failure leaves the network inactive.
pub fn shutdown() -> Result<(), &'static str> {
    let Some(state) = NET.lock().take() else {
        return Err("network is not running");
    };
    state.nic.shutdown()?;
    SELFTEST_PASSED.store(false, Ordering::SeqCst);
    crate::kprintln!(
        "net: stopped ({} DMA frame(s) released)",
        crate::e1000::DMA_PAGE_TOTAL
    );
    Ok(())
}

/// Boot selftest: resolve the gateway with ARP, then one ICMP echo. Logs the
/// pinned `net selftest:` line; never fatal.
pub fn selftest() {
    if NET.lock().is_none() {
        crate::kprintln!("net selftest: skipped (no NIC)");
        return;
    }
    if resolve(GATEWAY, ARP_ATTEMPTS).is_none() {
        crate::kprintln!("net selftest: no gateway reply (arp timeout)");
        return;
    }
    for sequence in 1..=u16::from(ECHO_ATTEMPTS) {
        let start = read_tsc();
        if request_echo(GATEWAY, sequence).is_err() {
            continue;
        }
        if await_echo(GATEWAY, sequence, ECHO_ATTEMPT_TICKS).is_some() {
            crate::kprintln!(
                "net selftest: passed (arp resolved, icmp echo reply in {} ms)",
                elapsed_ms_since(start)
            );
            SELFTEST_PASSED.store(true, Ordering::SeqCst);
            return;
        }
    }
    crate::kprintln!("net selftest: no gateway reply (echo timeout)");
}

/// Whether the boot selftest passed (controls the `,net` marker suffix).
pub fn selftest_passed() -> bool {
    SELFTEST_PASSED.load(Ordering::SeqCst)
}

/// One-line interface status for the shell.
pub fn status() -> Option<String> {
    let guard = NET.lock();
    let state = guard.as_ref()?;
    let (tx, rx, errors) = state.nic.counters();
    Some(alloc::format!(
        "net: mac {}, ip {}/{} gw {}; tx {} frames, rx {} frames, arp {}/{} entries, errors {}",
        state.nic.mac(),
        IP_ADDRESS,
        SUBNET_PREFIX,
        GATEWAY,
        tx,
        rx,
        state.cache.len(),
        ARP_ENTRIES,
        errors
    ))
}

/// `ping <ip> [-c n]`. Prints replies and a summary; an unresolved ARP or a
/// missing NIC is reported as a plain line, not a shell error.
pub fn ping(args: &str) -> Result<(), &'static str> {
    let mut parts = args.split_whitespace();
    let address = parts.next().ok_or("usage: ping <ip> [-c n]")?;
    let target = parse_addr(address).ok_or("invalid IPv4 address")?;
    let mut count: u8 = 3;
    while let Some(flag) = parts.next() {
        if flag != "-c" {
            return Err("usage: ping <ip> [-c n]");
        }
        let value = parts.next().ok_or("usage: ping <ip> [-c n]")?;
        let parsed: u8 = value.parse().map_err(|_| "invalid count")?;
        if parsed == 0 || parsed > 10 {
            return Err("count must be 1-10");
        }
        count = parsed;
    }
    if NET.lock().is_none() {
        return Err("no network device");
    }
    if resolve(target, ARP_ATTEMPTS).is_none() {
        crate::kprintln!("ping: arp timeout for {}", target);
        return Ok(());
    }

    let mut sent = 0u32;
    let mut received = 0u32;
    for sequence in 1..=u16::from(count) {
        sent += 1;
        let start = read_tsc();
        if request_echo(target, sequence).is_err() {
            break;
        }
        if let Some(reply) = await_echo(target, sequence, ECHO_ATTEMPT_TICKS) {
            received += 1;
            crate::kprintln!(
                "reply from {}: seq={} ttl={} time={} ms",
                reply.source,
                sequence,
                reply.ttl,
                elapsed_ms_since(start)
            );
        }
    }
    let loss = (sent - received) * 100 / sent.max(1);
    crate::kprintln!("ping: {} sent, {} received, {}% loss", sent, received, loss);
    Ok(())
}

/// `udpecho <port>`: wait for one UDP datagram to `10.0.2.15:port` (up to
/// 10 s) and echo its payload back to the sender.
pub fn udpecho(args: &str) -> Result<(), &'static str> {
    let port: u16 = args.trim().parse().map_err(|_| "usage: udpecho <port>")?;
    if port == 0 {
        return Err("usage: udpecho <port>");
    }
    if NET.lock().is_none() {
        return Err("no network device");
    }
    crate::kprintln!(
        "udpecho: listening on {}:{} for one datagram",
        IP_ADDRESS,
        port
    );
    let armed = with_state(|state| {
        state.udpecho = Some(UdpechoJob {
            port,
            done: false,
            echoed: 0,
            peer: (Addr::UNSPECIFIED, 0),
        });
    })
    .is_some();
    if !armed {
        return Err("no network device");
    }

    let deadline = pit::ticks() + UDPECHO_TIMEOUT_TICKS;
    let mut result: Option<(usize, Addr, u16)> = None;
    while pit::ticks() <= deadline {
        let done = with_state(|state| {
            drain_rx(state);
            match state.udpecho {
                Some(ref job) if job.done => {
                    result = Some((job.echoed, job.peer.0, job.peer.1));
                    state.udpecho = None;
                    true
                }
                _ => false,
            }
        })
        .unwrap_or(true);
        if done {
            break;
        }
        core::hint::spin_loop();
    }
    with_state(|state| state.udpecho = None);

    match result {
        Some((bytes, peer, peer_port)) => {
            crate::kprintln!("udpecho: echoed {} bytes to {}:{}", bytes, peer, peer_port);
        }
        None => crate::kprintln!("udpecho: no datagram within 10 s"),
    }
    Ok(())
}

fn parse_addr(text: &str) -> Option<Addr> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in text.split('.') {
        if count == 4 {
            return None;
        }
        octets[count] = part.parse().ok()?;
        count += 1;
    }
    if count != 4 {
        return None;
    }
    Some(Addr(octets))
}

/// Run `f` with the network state while holding the lock for its duration.
/// The closure must not wait on hardware — poll loops wrap this per iteration.
fn with_state<R>(f: impl FnOnce(&mut NetState) -> R) -> Option<R> {
    let mut guard = NET.lock();
    let state = guard.as_mut()?;
    Some(f(state))
}

fn drain_rx(state: &mut NetState) {
    let mut buffer = [0u8; MAX_FRAME_BYTES];
    for _ in 0..RX_DRAIN_PER_POLL {
        let Some(length) = state.nic.receive_into(&mut buffer) else {
            return;
        };
        dispatch(state, &buffer[..length]);
    }
}

fn dispatch(state: &mut NetState, frame: &[u8]) {
    let Some(header) = eth::Header::parse(frame) else {
        return;
    };
    if !header.destination.is_broadcast() && header.destination != state.nic.mac() {
        return;
    }
    match header.ethertype {
        eth::ETHERTYPE_ARP => handle_arp(state, frame),
        eth::ETHERTYPE_IPV4 => handle_ipv4(state, header.source, frame),
        _ => {}
    }
}

fn handle_arp(state: &mut NetState, frame: &[u8]) {
    let Some(packet) = arp::Packet::parse(&frame[eth::HEADER_LEN..]) else {
        return;
    };
    if packet.sender_mac != MacAddr::ZERO && !packet.sender_ip.is_unspecified() {
        state
            .cache
            .insert(packet.sender_ip, packet.sender_mac, pit::ticks());
    }
    if packet.operation == arp::OPERATION_REQUEST && packet.target_ip == IP_ADDRESS {
        let _ = send_arp_reply(state, packet.sender_mac, packet.sender_ip);
    }
}

fn handle_ipv4(state: &mut NetState, source_mac: MacAddr, frame: &[u8]) {
    let Ok((ip, payload)) = ipv4::Header::parse(&frame[eth::HEADER_LEN..]) else {
        return;
    };
    if ip.destination != IP_ADDRESS && ip.destination != Addr::BROADCAST {
        return;
    }
    if source_mac != MacAddr::ZERO && !ip.source.is_unspecified() {
        state.cache.insert(ip.source, source_mac, pit::ticks());
    }
    match ip.protocol {
        ipv4::PROTOCOL_ICMP => handle_icmp(state, source_mac, &ip, payload),
        ipv4::PROTOCOL_UDP => handle_udp(state, source_mac, &ip, payload),
        _ => {}
    }
}

fn handle_icmp(state: &mut NetState, source_mac: MacAddr, ip: &ipv4::Header, payload: &[u8]) {
    let Some((message, data)) = icmp::Message::parse(payload) else {
        return;
    };
    match message.type_ {
        icmp::TYPE_ECHO_REQUEST => {
            let _ = send_echo_reply(
                state,
                source_mac,
                ip.source,
                message.identifier,
                message.sequence,
                data,
            );
        }
        icmp::TYPE_ECHO_REPLY if message.identifier == ECHO_IDENTIFIER => {
            state.last_echo = Some(IcmpReply {
                source: ip.source,
                ttl: ip.ttl,
                sequence: message.sequence,
            });
        }
        _ => {}
    }
}

fn handle_udp(state: &mut NetState, source_mac: MacAddr, ip: &ipv4::Header, payload: &[u8]) {
    let Some((header, data)) = udp::Header::parse(payload) else {
        return;
    };
    if !udp::verify_checksum(payload, ip.source, ip.destination) {
        return;
    }
    if data.len() > UDPECHO_MAX_PAYLOAD {
        return;
    }
    let port = match state.udpecho {
        Some(ref job) if !job.done => job.port,
        _ => return,
    };
    if header.destination_port != port {
        return;
    }
    if send_udp_echo_reply(state, source_mac, ip.source, header.source_port, data, port).is_ok()
        && let Some(job) = state.udpecho.as_mut()
    {
        job.done = true;
        job.echoed = data.len();
        job.peer = (ip.source, header.source_port);
    }
}

fn send_arp_request(state: &mut NetState, target: Addr) -> Result<(), &'static str> {
    let mut frame = [0u8; 60];
    eth::Header::write(
        &mut frame,
        MacAddr::BROADCAST,
        state.nic.mac(),
        eth::ETHERTYPE_ARP,
    )
    .ok_or("frame buffer too small")?;
    arp::Packet::write_request(
        &mut frame[eth::HEADER_LEN..],
        state.nic.mac(),
        IP_ADDRESS,
        target,
    )
    .ok_or("frame buffer too small")?;
    state.nic.transmit(&frame)
}

fn send_arp_reply(
    state: &mut NetState,
    destination_mac: MacAddr,
    destination_ip: Addr,
) -> Result<(), &'static str> {
    let mut frame = [0u8; 60];
    eth::Header::write(
        &mut frame,
        destination_mac,
        state.nic.mac(),
        eth::ETHERTYPE_ARP,
    )
    .ok_or("frame buffer too small")?;
    arp::Packet::write_reply(
        &mut frame[eth::HEADER_LEN..],
        state.nic.mac(),
        IP_ADDRESS,
        destination_mac,
        destination_ip,
    )
    .ok_or("frame buffer too small")?;
    state.nic.transmit(&frame)
}

fn send_echo_request(
    state: &mut NetState,
    destination_mac: MacAddr,
    target: Addr,
    sequence: u16,
) -> Result<(), &'static str> {
    const ECHO_PAYLOAD: usize = 32;
    let icmp_len = icmp::HEADER_LEN + ECHO_PAYLOAD;
    let ip_len = ipv4::HEADER_LEN + icmp_len;
    let total = eth::HEADER_LEN + ip_len;
    let mut frame = [0u8; 96];
    if total > frame.len() {
        return Err("frame buffer too small");
    }
    eth::Header::write(
        &mut frame,
        destination_mac,
        state.nic.mac(),
        eth::ETHERTYPE_IPV4,
    )
    .ok_or("frame buffer too small")?;
    ipv4::Header::write(
        &mut frame[eth::HEADER_LEN..],
        IP_ADDRESS,
        target,
        ipv4::PROTOCOL_ICMP,
        sequence,
        icmp_len,
    )
    .ok_or("frame buffer too small")?;
    let icmp_slice = &mut frame[eth::HEADER_LEN + ipv4::HEADER_LEN..total];
    for (index, byte) in icmp_slice[icmp::HEADER_LEN..].iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37).wrapping_add(11);
    }
    icmp::Message::write(
        icmp_slice,
        icmp::TYPE_ECHO_REQUEST,
        ECHO_IDENTIFIER,
        sequence,
        ECHO_PAYLOAD,
    )
    .ok_or("frame buffer too small")?;
    state.nic.transmit(&frame[..total])
}

fn send_echo_reply(
    state: &mut NetState,
    destination_mac: MacAddr,
    destination_ip: Addr,
    identifier: u16,
    sequence: u16,
    data: &[u8],
) -> Result<(), &'static str> {
    let icmp_len = icmp::HEADER_LEN + data.len();
    let ip_len = ipv4::HEADER_LEN + icmp_len;
    let total = eth::HEADER_LEN + ip_len;
    if total > MAX_FRAME_BYTES {
        return Err("echo reply too large");
    }
    let mut frame = [0u8; MAX_FRAME_BYTES];
    eth::Header::write(
        &mut frame,
        destination_mac,
        state.nic.mac(),
        eth::ETHERTYPE_IPV4,
    )
    .ok_or("frame buffer too small")?;
    ipv4::Header::write(
        &mut frame[eth::HEADER_LEN..],
        IP_ADDRESS,
        destination_ip,
        ipv4::PROTOCOL_ICMP,
        0,
        icmp_len,
    )
    .ok_or("frame buffer too small")?;
    let icmp_slice = &mut frame[eth::HEADER_LEN + ipv4::HEADER_LEN..total];
    icmp_slice[icmp::HEADER_LEN..].copy_from_slice(data);
    icmp::Message::write(
        icmp_slice,
        icmp::TYPE_ECHO_REPLY,
        identifier,
        sequence,
        data.len(),
    )
    .ok_or("frame buffer too small")?;
    state.nic.transmit(&frame[..total.max(60)])
}

fn send_udp_echo_reply(
    state: &mut NetState,
    destination_mac: MacAddr,
    destination_ip: Addr,
    destination_port: u16,
    data: &[u8],
    source_port: u16,
) -> Result<(), &'static str> {
    let udp_len = udp::HEADER_LEN + data.len();
    let ip_len = ipv4::HEADER_LEN + udp_len;
    let total = eth::HEADER_LEN + ip_len;
    if total > MAX_FRAME_BYTES {
        return Err("udp echo too large");
    }
    let mut frame = [0u8; MAX_FRAME_BYTES];
    eth::Header::write(
        &mut frame,
        destination_mac,
        state.nic.mac(),
        eth::ETHERTYPE_IPV4,
    )
    .ok_or("frame buffer too small")?;
    ipv4::Header::write(
        &mut frame[eth::HEADER_LEN..],
        IP_ADDRESS,
        destination_ip,
        ipv4::PROTOCOL_UDP,
        0,
        udp_len,
    )
    .ok_or("frame buffer too small")?;
    let datagram = &mut frame[eth::HEADER_LEN + ipv4::HEADER_LEN..total];
    udp::Header::write(datagram, source_port, destination_port).ok_or("frame buffer too small")?;
    datagram[udp::HEADER_LEN..].copy_from_slice(data);
    udp::write_checksum(datagram, IP_ADDRESS, destination_ip).ok_or("frame buffer too small")?;
    state.nic.transmit(&frame[..total.max(60)])
}

/// ARP-resolve `target`, trying `attempts` requests one second apart.
fn resolve(target: Addr, attempts: u8) -> Option<MacAddr> {
    if let Some(mac) = with_state(|state| state.cache.lookup(target, pit::ticks())).flatten() {
        return Some(mac);
    }
    for _ in 0..attempts {
        let _ = with_state(|state| send_arp_request(state, target));
        let deadline = pit::ticks() + ARP_ATTEMPT_TICKS;
        while pit::ticks() <= deadline {
            let found = with_state(|state| {
                drain_rx(state);
                state.cache.lookup(target, pit::ticks())
            })
            .flatten();
            if let Some(mac) = found {
                return Some(mac);
            }
            core::hint::spin_loop();
        }
    }
    None
}

fn request_echo(target: Addr, sequence: u16) -> Result<(), &'static str> {
    match with_state(|state| {
        let mac = state
            .cache
            .lookup(target, pit::ticks())
            .ok_or("arp unresolved")?;
        send_echo_request(state, mac, target, sequence)
    }) {
        Some(result) => result,
        None => Err("no network device"),
    }
}

fn await_echo(target: Addr, sequence: u16, timeout_ticks: u64) -> Option<IcmpReply> {
    let deadline = pit::ticks() + timeout_ticks;
    while pit::ticks() <= deadline {
        let reply = with_state(|state| {
            drain_rx(state);
            state.last_echo.take()
        })
        .flatten();
        if let Some(reply) = reply
            && reply.source == target
            && reply.sequence == sequence
        {
            return Some(reply);
        }
        core::hint::spin_loop();
    }
    None
}

fn elapsed_ms_since(start_tsc: u64) -> u64 {
    let frequency = TSC_FREQUENCY.load(Ordering::SeqCst);
    if frequency < 1_000_000 {
        return 0;
    }
    read_tsc().wrapping_sub(start_tsc) / (frequency / 1000)
}
