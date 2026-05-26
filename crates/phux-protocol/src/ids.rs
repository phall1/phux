//! Stable identifiers used across the protocol.
//!
//! All IDs are opaque `u32` values, monotonically allocated by the server.
//! IDs are stable for the server's lifetime and are not reused after the
//! entity is destroyed.

macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);

        impl $name {
            /// Construct from a raw `u32`.
            #[must_use]
            pub const fn new(raw: u32) -> Self {
                Self(raw)
            }

            /// Inner raw value.
            #[must_use]
            pub const fn get(self) -> u32 {
                self.0
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

id_type!(
    /// Identifier for a session within a server.
    SessionId
);
id_type!(
    /// Identifier for a window within a server.
    WindowId
);
id_type!(
    /// Identifier for a terminal within a server.
    TerminalId
);
id_type!(
    /// Identifier for a currently-connected client.
    ClientId
);

/// Identifier for a terminal frame. Monotonically increasing per terminal; `0`
/// is the empty initial frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct FrameId(pub u64);

impl FrameId {
    /// The empty initial frame, before any output.
    pub const ZERO: Self = Self(0);

    /// Advance to the next frame.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

impl core::fmt::Display for FrameId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "FrameId({})", self.0)
    }
}
