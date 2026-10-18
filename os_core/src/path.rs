//! Absolute path syntax for the in-memory filesystem (Phase 6).
//!
//! The kernel VFS resolves paths with these rules, and this module is
//! host-tested so the kernel only adds storage on top of known-good parsing:
//!
//! * paths are absolute — they must start with `/`;
//! * `/` alone is the root directory and has zero components;
//! * components are separated by single `/` characters (no empty components,
//!   so `//` and trailing `/` are rejected);
//! * `.` and `..` are rejected (no hidden normalisation in this milestone);
//! * components are 1..=[`MAX_COMPONENT`] printable ASCII bytes without `/`,
//!   spaces or control characters;
//! * the whole path is at most [`MAX_PATH`] bytes.

/// Maximum accepted path length in bytes.
pub const MAX_PATH: usize = 128;
/// Maximum accepted single-component length in bytes.
pub const MAX_COMPONENT: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathError {
    Empty,
    NotAbsolute,
    TooLong,
    EmptyComponent,
    ComponentTooLong,
    BadCharacter,
}

impl PathError {
    pub fn label(&self) -> &'static str {
        match self {
            PathError::Empty => "empty path",
            PathError::NotAbsolute => "path is not absolute",
            PathError::TooLong => "path is too long",
            PathError::EmptyComponent => "path contains an empty component",
            PathError::ComponentTooLong => "path component is too long",
            PathError::BadCharacter => "path contains an unsupported character",
        }
    }
}

/// Check a single path component against the component rules (non-empty,
/// at most [`MAX_COMPONENT`] bytes, printable ASCII without `/`). Shared with
/// the checkpoint format so both enforce identical names.
pub fn component_is_valid(component: &str) -> Result<(), PathError> {
    if component.is_empty() {
        return Err(PathError::EmptyComponent);
    }
    component_is_valid_inner(component)
}

fn component_is_valid_inner(component: &str) -> Result<(), PathError> {
    if component.len() > MAX_COMPONENT {
        return Err(PathError::ComponentTooLong);
    }
    for byte in component.bytes() {
        let printable = (0x21..=0x7e).contains(&byte) && byte != b'/';
        if !printable {
            return Err(PathError::BadCharacter);
        }
    }
    Ok(())
}

/// Validate an absolute path. Returns the number of components on success so
/// callers can size fixed buffers without re-walking the string.
pub fn validate(path: &str) -> Result<usize, PathError> {
    if path.is_empty() {
        return Err(PathError::Empty);
    }
    if path.len() > MAX_PATH {
        return Err(PathError::TooLong);
    }
    let Some(rest) = path.strip_prefix('/') else {
        return Err(PathError::NotAbsolute);
    };
    if rest.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for component in rest.split('/') {
        if component.is_empty() {
            return Err(PathError::EmptyComponent);
        }
        if component == "." || component == ".." {
            return Err(PathError::BadCharacter);
        }
        component_is_valid(component)?;
        count += 1;
    }
    Ok(count)
}

/// Iterator over the components of a validated absolute path. The root path
/// yields no components.
#[derive(Clone, Copy, Debug)]
pub struct Components<'a> {
    rest: &'a str,
}

impl<'a> Components<'a> {
    pub fn new(path: &'a str) -> Result<Self, PathError> {
        validate(path)?;
        Ok(Self {
            rest: path.strip_prefix('/').unwrap_or(""),
        })
    }
}

impl<'a> Iterator for Components<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        if self.rest.is_empty() {
            return None;
        }
        match self.rest.find('/') {
            Some(index) => {
                let (component, rest) = self.rest.split_at(index);
                self.rest = &rest[1..];
                Some(component)
            }
            None => {
                let component = self.rest;
                self.rest = "";
                Some(component)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(path: &str) -> Vec<&str> {
        Components::new(path).expect("valid path").collect()
    }

    #[test]
    fn root_is_valid_and_has_no_components() {
        assert_eq!(validate("/"), Ok(0));
        assert_eq!(parts("/"), Vec::<&str>::new());
    }

    #[test]
    fn simple_and_deep_paths_split_correctly() {
        assert_eq!(parts("/etc/motd"), vec!["etc", "motd"]);
        assert_eq!(parts("/a/b/c/d/e"), vec!["a", "b", "c", "d", "e"]);
        assert_eq!(validate("/etc/motd"), Ok(2));
    }

    #[test]
    fn empty_and_relative_paths_are_rejected() {
        assert_eq!(validate(""), Err(PathError::Empty));
        assert_eq!(validate("etc"), Err(PathError::NotAbsolute));
        assert_eq!(validate("./etc"), Err(PathError::NotAbsolute));
    }

    #[test]
    fn empty_components_are_rejected() {
        assert_eq!(validate("//"), Err(PathError::EmptyComponent));
        assert_eq!(validate("/etc//motd"), Err(PathError::EmptyComponent));
        assert_eq!(validate("/etc/"), Err(PathError::EmptyComponent));
    }

    #[test]
    fn dot_and_dotdot_are_rejected() {
        assert_eq!(validate("/."), Err(PathError::BadCharacter));
        assert_eq!(validate("/.."), Err(PathError::BadCharacter));
        assert_eq!(validate("/etc/../motd"), Err(PathError::BadCharacter));
    }

    #[test]
    fn component_length_is_bounded() {
        let long = "a".repeat(MAX_COMPONENT);
        assert!(validate(&format!("/{long}")).is_ok());
        let too_long = "a".repeat(MAX_COMPONENT + 1);
        assert_eq!(
            validate(&format!("/{too_long}")),
            Err(PathError::ComponentTooLong)
        );
    }

    #[test]
    fn total_length_is_bounded() {
        let name = "a".repeat(MAX_COMPONENT);
        let mut path = String::new();
        for _ in 0..8 {
            path.push('/');
            path.push_str(&name);
        }
        assert!(path.len() > MAX_PATH);
        assert_eq!(validate(&path), Err(PathError::TooLong));
    }

    #[test]
    fn bad_characters_are_rejected() {
        assert_eq!(validate("/et c"), Err(PathError::BadCharacter));
        assert_eq!(validate("/tab\ttab"), Err(PathError::BadCharacter));
        assert_eq!(validate("/caf\u{e9}"), Err(PathError::BadCharacter));
        assert_eq!(validate("/new\nline"), Err(PathError::BadCharacter));
        assert_eq!(validate("/delete\x7f"), Err(PathError::BadCharacter));
    }

    #[test]
    fn dots_inside_names_are_allowed() {
        assert_eq!(parts("/etc/motd.txt"), vec!["etc", "motd.txt"]);
        assert_eq!(parts("/a.b/c.d"), vec!["a.b", "c.d"]);
    }
}
