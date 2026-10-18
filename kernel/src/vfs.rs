//! In-memory filesystem (Phase 6, first slice).
//!
//! A single tree of nodes lives in the kernel heap: index 0 is the root
//! directory; every other node stores its name, kind, inline file data and its
//! parent/children indices. There is no on-disk state in this milestone — that
//! arrives with the block device and the persistent-format work.
//!
//! All path parsing goes through `os_core::path` (host-tested: absolute-only,
//! no `.`/`..`, bounded component and total lengths). Node data is bounded by
//! [`MAX_FILE_BYTES`] and the tree is bounded by [`MAX_NODES`] so a shell
//! script cannot exhaust the fixed 8 MiB kernel heap.
//!
//! Access is serialised by an `IrqSpinLock`; callers use [`with`] to run a
//! closure with the locked tree so no lock guard escapes this module.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use os_core::path::{self, PathError};
use os_core::pofs::{self, NodeRecord};

use crate::sync::IrqSpinLock;

/// Maximum number of nodes (files + directories) in the tree.
pub const MAX_NODES: usize = 64;
/// Maximum size of one file's inline data.
pub const MAX_FILE_BYTES: usize = 8192;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NodeKind {
    Dir,
    File,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VfsError {
    InvalidPath(PathError),
    NotFound,
    Exists,
    NotADirectory,
    IsADirectory,
    NotEmpty,
    TooManyNodes,
    FileTooBig,
    ReadOnly,
    Corrupt,
}

impl VfsError {
    pub fn label(&self) -> &'static str {
        match self {
            VfsError::InvalidPath(error) => error.label(),
            VfsError::NotFound => "no such file or directory",
            VfsError::Exists => "already exists",
            VfsError::NotADirectory => "not a directory",
            VfsError::IsADirectory => "is a directory",
            VfsError::NotEmpty => "directory is not empty",
            VfsError::TooManyNodes => "filesystem is full",
            VfsError::FileTooBig => "file is too large",
            VfsError::ReadOnly => "this node cannot be removed",
            VfsError::Corrupt => "checkpoint data is invalid",
        }
    }
}

struct Node {
    name: String,
    kind: NodeKind,
    data: Vec<u8>,
    parent: Option<usize>,
    children: Vec<usize>,
}

pub struct Vfs {
    nodes: Vec<Node>,
}

impl Vfs {
    pub fn new() -> Self {
        Self {
            nodes: alloc::vec![Node {
                name: String::new(),
                kind: NodeKind::Dir,
                data: Vec::new(),
                parent: None,
                children: Vec::new(),
            }],
        }
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn file_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|node| node.kind == NodeKind::File)
            .count()
    }

    fn child_named(&self, directory: usize, name: &str) -> Option<usize> {
        self.nodes[directory]
            .children
            .iter()
            .copied()
            .find(|&child| self.nodes[child].name == name)
    }

    /// Resolve an absolute path to a node index.
    fn resolve(&self, raw_path: &str) -> Result<usize, VfsError> {
        let components = path::Components::new(raw_path).map_err(VfsError::InvalidPath)?;
        let mut current = 0;
        for component in components {
            if self.nodes[current].kind != NodeKind::Dir {
                return Err(VfsError::NotADirectory);
            }
            current = self
                .child_named(current, component)
                .ok_or(VfsError::NotFound)?;
        }
        Ok(current)
    }

    /// Resolve the parent directory of a (not necessarily existing) node and
    /// return it together with the final component name.
    fn resolve_parent<'a>(&self, raw_path: &'a str) -> Result<(usize, &'a str), VfsError> {
        let components = path::Components::new(raw_path).map_err(VfsError::InvalidPath)?;
        let mut current = 0;
        let mut previous: Option<&str> = None;
        for component in components {
            if let Some(name) = previous {
                if self.nodes[current].kind != NodeKind::Dir {
                    return Err(VfsError::NotADirectory);
                }
                current = self.child_named(current, name).ok_or(VfsError::NotFound)?;
            }
            previous = Some(component);
        }
        match previous {
            Some(name) => Ok((current, name)),
            None => Err(VfsError::InvalidPath(PathError::Empty)),
        }
    }

    fn push_node(
        &mut self,
        parent: usize,
        name: &str,
        kind: NodeKind,
        data: &[u8],
    ) -> Result<usize, VfsError> {
        if self.nodes.len() >= MAX_NODES {
            return Err(VfsError::TooManyNodes);
        }
        if data.len() > MAX_FILE_BYTES {
            return Err(VfsError::FileTooBig);
        }
        self.nodes.push(Node {
            name: String::from(name),
            kind,
            data: data.to_vec(),
            parent: Some(parent),
            children: Vec::new(),
        });
        let index = self.nodes.len() - 1;
        self.nodes[parent].children.push(index);
        Ok(index)
    }

    pub fn create_file(&mut self, raw_path: &str, data: &[u8]) -> Result<(), VfsError> {
        let (parent, name) = self.resolve_parent(raw_path)?;
        if self.child_named(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        self.push_node(parent, name, NodeKind::File, data)?;
        mark_dirty();
        Ok(())
    }

    pub fn mkdir(&mut self, raw_path: &str) -> Result<(), VfsError> {
        let (parent, name) = self.resolve_parent(raw_path)?;
        if self.child_named(parent, name).is_some() {
            return Err(VfsError::Exists);
        }
        self.push_node(parent, name, NodeKind::Dir, &[])?;
        mark_dirty();
        Ok(())
    }

    /// Create a file or replace its contents (the shell's `write` contract).
    pub fn write_file(&mut self, raw_path: &str, data: &[u8]) -> Result<(), VfsError> {
        if data.len() > MAX_FILE_BYTES {
            return Err(VfsError::FileTooBig);
        }
        let (parent, name) = self.resolve_parent(raw_path)?;
        match self.child_named(parent, name) {
            Some(existing) => {
                if self.nodes[existing].kind != NodeKind::File {
                    return Err(VfsError::IsADirectory);
                }
                self.nodes[existing].data.clear();
                self.nodes[existing].data.extend_from_slice(data);
                mark_dirty();
                Ok(())
            }
            None => {
                self.push_node(parent, name, NodeKind::File, data)?;
                mark_dirty();
                Ok(())
            }
        }
    }

    /// Append to an existing file; returns the new size.
    pub fn append_file(&mut self, raw_path: &str, data: &[u8]) -> Result<usize, VfsError> {
        let index = self.resolve(raw_path)?;
        if self.nodes[index].kind != NodeKind::File {
            return Err(VfsError::IsADirectory);
        }
        if self.nodes[index].data.len() + data.len() > MAX_FILE_BYTES {
            return Err(VfsError::FileTooBig);
        }
        self.nodes[index].data.extend_from_slice(data);
        mark_dirty();
        Ok(self.nodes[index].data.len())
    }

    pub fn read_file(&self, raw_path: &str) -> Result<&[u8], VfsError> {
        let index = self.resolve(raw_path)?;
        if self.nodes[index].kind != NodeKind::File {
            return Err(VfsError::IsADirectory);
        }
        Ok(&self.nodes[index].data)
    }

    /// Remove a file or an empty directory. The root is never removable.
    pub fn remove(&mut self, raw_path: &str) -> Result<(), VfsError> {
        let index = self.resolve(raw_path)?;
        if index == 0 {
            return Err(VfsError::ReadOnly);
        }
        if !self.nodes[index].children.is_empty() {
            return Err(VfsError::NotEmpty);
        }
        let parent = self.nodes[index].parent.ok_or(VfsError::ReadOnly)?;
        self.nodes[parent].children.retain(|&child| child != index);

        // Swap-remove and fix up every index that pointed at the tail.
        let last = self.nodes.len() - 1;
        self.nodes.swap_remove(index);
        if index != last {
            for node in &mut self.nodes {
                for child in &mut node.children {
                    if *child == last {
                        *child = index;
                    }
                }
            }
        }
        mark_dirty();
        Ok(())
    }

    /// List a directory as `(name, kind, size)` triples (size = file bytes,
    /// or the child count for directories).
    pub fn list(&self, raw_path: &str) -> Result<Vec<(String, NodeKind, usize)>, VfsError> {
        let index = self.resolve(raw_path)?;
        if self.nodes[index].kind != NodeKind::Dir {
            return Err(VfsError::NotADirectory);
        }
        let mut entries = Vec::with_capacity(self.nodes[index].children.len());
        for &child in &self.nodes[index].children {
            let node = &self.nodes[child];
            let size = match node.kind {
                NodeKind::File => node.data.len(),
                NodeKind::Dir => node.children.len(),
            };
            entries.push((node.name.clone(), node.kind, size));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    /// Kind and size of a node (file bytes / directory child count).
    pub fn stat(&self, raw_path: &str) -> Result<(NodeKind, usize), VfsError> {
        let index = self.resolve(raw_path)?;
        let node = &self.nodes[index];
        let size = match node.kind {
            NodeKind::File => node.data.len(),
            NodeKind::Dir => node.children.len(),
        };
        Ok((node.kind, size))
    }

    /// Serialise the tree for the checkpoint format: depth-first pre-order
    /// with stream-remapped parent indices, so every parent precedes its
    /// children and sibling order is preserved (`remove()` uses swap-remove,
    /// so arena order alone is not a valid stream order).
    pub fn export(&self) -> Vec<NodeRecord> {
        let mut records = Vec::with_capacity(self.nodes.len());
        let mut stack = alloc::vec![(0usize, pofs::NO_PARENT)];
        while let Some((index, parent)) = stack.pop() {
            let node = &self.nodes[index];
            let stream_index = records.len() as u32;
            records.push(NodeRecord {
                kind: match node.kind {
                    NodeKind::Dir => pofs::KIND_DIR,
                    NodeKind::File => pofs::KIND_FILE,
                },
                parent,
                name: node.name.clone(),
                data: node.data.clone(),
            });
            for &child in node.children.iter().rev() {
                stack.push((child, stream_index));
            }
        }
        records
    }

    /// Replace the whole tree with records in checkpoint stream order. The
    /// decoder in `os_core::pofs` is the primary validator; this re-checks
    /// the storage limits before allocating the new arena.
    pub fn replace(&mut self, records: &[NodeRecord]) -> Result<(), VfsError> {
        if records.is_empty() || records.len() > MAX_NODES {
            return Err(VfsError::TooManyNodes);
        }
        let root = &records[0];
        if root.kind != pofs::KIND_DIR
            || root.parent != pofs::NO_PARENT
            || !root.name.is_empty()
            || !root.data.is_empty()
        {
            return Err(VfsError::Corrupt);
        }

        let mut nodes = Vec::with_capacity(records.len());
        nodes.push(Node {
            name: String::new(),
            kind: NodeKind::Dir,
            data: Vec::new(),
            parent: None,
            children: Vec::new(),
        });
        for (index, record) in records.iter().enumerate().skip(1) {
            let parent = record.parent as usize;
            if record.parent == pofs::NO_PARENT || parent >= index {
                return Err(VfsError::Corrupt);
            }
            let kind = match record.kind {
                pofs::KIND_DIR => NodeKind::Dir,
                pofs::KIND_FILE => NodeKind::File,
                _ => return Err(VfsError::Corrupt),
            };
            if record.data.len() > MAX_FILE_BYTES {
                return Err(VfsError::FileTooBig);
            }
            if kind == NodeKind::Dir && !record.data.is_empty() {
                return Err(VfsError::Corrupt);
            }
            if path::component_is_valid(&record.name).is_err() {
                return Err(VfsError::InvalidPath(PathError::BadCharacter));
            }
            nodes.push(Node {
                name: record.name.clone(),
                kind,
                data: record.data.clone(),
                parent: Some(parent),
                children: Vec::new(),
            });
            nodes[parent].children.push(index);
        }
        self.nodes = nodes;
        clear_dirty();
        Ok(())
    }

    /// Create the initial tree used by the shell and the boot self-test.
    pub fn seed(&mut self) -> Result<(), VfsError> {
        self.mkdir("/etc")?;
        self.mkdir("/tmp")?;
        self.create_file("/etc/motd", MOTD)?;
        self.create_file("/etc/version", VERSION_FILE)?;
        Ok(())
    }
}

impl Default for Vfs {
    fn default() -> Self {
        Self::new()
    }
}

pub const MOTD: &[u8] = b"Welcome to PROJECT_OS.\nThis is an in-memory filesystem; nothing persists across reboots yet.\n";
pub const VERSION_FILE: &[u8] = b"PROJECT_OS 0.1.0 (in-memory VFS)\n";

static VFS: IrqSpinLock<Option<Vfs>> = IrqSpinLock::new(None);

/// Set by every successful mutation of the live tree; the persistence layer
/// owns flushing it to the checkpoint image.
static DIRTY: AtomicBool = AtomicBool::new(false);

fn mark_dirty() {
    DIRTY.store(true, Ordering::Relaxed);
}

/// True when a mutation happened since the last successful checkpoint save.
pub fn dirty() -> bool {
    DIRTY.load(Ordering::Relaxed)
}

/// Cleared by the persistence layer after a successful save or a load.
pub fn clear_dirty() {
    DIRTY.store(false, Ordering::Relaxed);
}

pub fn init() -> Result<(), &'static str> {
    let mut slot = VFS.lock();
    if slot.is_some() {
        return Err("vfs: already initialised");
    }
    let mut vfs = Vfs::new();
    vfs.seed().map_err(|_| "vfs: seeding failed")?;
    *slot = Some(vfs);
    Ok(())
}

/// Run `operation` with the locked filesystem.
pub fn with<R>(operation: impl FnOnce(&mut Vfs) -> R) -> Result<R, &'static str> {
    let mut slot = VFS.lock();
    let vfs = slot.as_mut().ok_or("vfs: not initialised")?;
    Ok(operation(vfs))
}

fn expect(condition: bool, message: &'static str) -> Result<(), &'static str> {
    if condition { Ok(()) } else { Err(message) }
}

/// Boot-time self-test: exercises the full API surface and the error paths.
pub fn self_test() -> Result<(usize, usize), &'static str> {
    with(|vfs| -> Result<(), &'static str> {
        if vfs.node_count() != 5 {
            return Err("seed tree should have five nodes");
        }

        vfs.create_file("/tmp/a.txt", b"alpha")
            .map_err(|_| "create /tmp/a.txt failed")?;
        expect(
            vfs.read_file("/tmp/a.txt")
                .map_err(|_| "read a.txt failed")?
                == b"alpha",
            "a.txt content mismatch",
        )?;

        let size = vfs
            .append_file("/tmp/a.txt", b"-beta")
            .map_err(|_| "append failed")?;
        expect(size == 10, "append size mismatch")?;
        expect(
            vfs.read_file("/tmp/a.txt").map_err(|_| "re-read failed")? == b"alpha-beta",
            "append content mismatch",
        )?;

        vfs.write_file("/tmp/a.txt", b"gamma")
            .map_err(|_| "overwrite failed")?;
        expect(
            vfs.read_file("/tmp/a.txt")
                .map_err(|_| "read after overwrite failed")?
                == b"gamma",
            "overwrite content mismatch",
        )?;

        vfs.write_file("/tmp/created.txt", b"hello")
            .map_err(|_| "write_file must create a missing file")?;
        expect(
            vfs.read_file("/tmp/created.txt")
                .map_err(|_| "read of write_file-created file failed")?
                == b"hello",
            "write_file create content mismatch",
        )?;
        vfs.remove("/tmp/created.txt")
            .map_err(|_| "cleanup of created file failed")?;

        vfs.mkdir("/tmp/sub").map_err(|_| "mkdir /tmp/sub failed")?;
        vfs.create_file("/tmp/sub/f", b"x")
            .map_err(|_| "create /tmp/sub/f failed")?;
        let entries = vfs.list("/tmp").map_err(|_| "list /tmp failed")?;
        expect(entries.len() == 2, "list /tmp should show two entries")?;

        expect(
            vfs.remove("/tmp/sub") == Err(VfsError::NotEmpty),
            "removing a non-empty directory must fail",
        )?;
        vfs.remove("/tmp/sub/f")
            .map_err(|_| "remove /tmp/sub/f failed")?;
        vfs.remove("/tmp/sub")
            .map_err(|_| "remove /tmp/sub failed")?;

        expect(
            vfs.create_file("/tmp/a.txt", b"x") == Err(VfsError::Exists),
            "duplicate create must fail",
        )?;
        expect(
            vfs.create_file("/missing/f", b"x") == Err(VfsError::NotFound),
            "create under a missing parent must fail",
        )?;
        expect(
            vfs.write_file("/etc", b"x") == Err(VfsError::IsADirectory),
            "writing to a directory must fail",
        )?;
        expect(
            vfs.read_file("/no-such-file") == Err(VfsError::NotFound),
            "reading a missing file must fail",
        )?;
        expect(
            vfs.list("/etc/motd") == Err(VfsError::NotADirectory),
            "listing a file must fail",
        )?;
        expect(
            matches!(vfs.read_file("../escape"), Err(VfsError::InvalidPath(_))),
            "relative traversal must be rejected",
        )?;
        expect(
            vfs.remove("/") == Err(VfsError::ReadOnly),
            "the root must not be removable",
        )?;
        let big = alloc::vec![0u8; MAX_FILE_BYTES + 1];
        expect(
            vfs.create_file("/tmp/big", &big) == Err(VfsError::FileTooBig),
            "oversized files must be rejected",
        )?;

        let (kind, size) = vfs.stat("/etc/motd").map_err(|_| "stat failed")?;
        expect(
            kind == NodeKind::File && size == MOTD.len(),
            "motd stat mismatch",
        )?;
        Ok(())
    })??;

    let counts = with(|vfs| (vfs.node_count(), vfs.file_count()))?;
    Ok(counts)
}
