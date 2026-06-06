use std::collections::{HashMap, HashSet};

use phux_protocol::ids::{CollectionId, TerminalId as WireTerminalId};
use phux_protocol::wire::frame::Scope;

use super::client::ClientId;

/// Per-scope K/V store for L3 metadata (SPEC §7.4 / §11.L3) plus the
/// matching subscription registry.
///
/// Held inside [`super::ServerState`] but lifted into its own type so the
/// subscribe / set / delete / list operations live in a focused
/// surface — easier to test, easier to reason about ordering invariants,
/// and a natural home for the per-key size cap once that lands.
#[derive(Debug, Default)]
pub struct MetadataStore {
    /// Per-Terminal key → value. Cleared when the Terminal closes (the
    /// L1 lifecycle that owns the Terminal).
    terminal: HashMap<WireTerminalId, HashMap<String, Vec<u8>>>,
    /// Per-Collection key → value.
    collection: HashMap<CollectionId, HashMap<String, Vec<u8>>>,
    /// Global key → value.
    global: HashMap<String, Vec<u8>>,
    /// Active subscriptions: a flat set of `(client, scope, key)` tuples.
    /// Lookup on broadcast is linear in the number of subscriptions; that
    /// is acceptable while subscriptions are sparse (handful per client).
    /// A future ticket may switch this to a `HashMap<(scope, key), Vec<ClientId>>`
    /// if the dispatch path shows up in flame graphs.
    subscriptions: HashSet<(ClientId, Scope, String)>,
}

/// Outcome of a `SET_METADATA` call.
///
/// `Unchanged` means the key already held an identical value, so the
/// server SHOULD suppress the `METADATA_CHANGED` broadcast (it's a noop
/// from every subscriber's perspective).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetadataSetOutcome {
    /// Key did not exist or held a different value; value was written.
    Changed,
    /// Key already held the identical value; no broadcast needed.
    Unchanged,
}

/// Outcome of [`super::ServerState::rename_session`], mapping the three terminal
/// cases of a `RENAME_SESSION` to the wire replies the server issues.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameOutcome {
    /// The session was renamed (or already bore the requested name); reply
    /// `COMMAND_RESULT { Ok }`.
    Renamed,
    /// No session matched the current name; reply `SESSION_NOT_FOUND`.
    NotFound,
    /// Another live session already holds the requested name; reply
    /// `INVALID_COMMAND` (the code `CREATE_SESSION` uses for a taken name).
    NameTaken,
}

impl MetadataStore {
    /// Get the value at `(scope, key)`, if any.
    #[must_use]
    pub fn get(&self, scope: &Scope, key: &str) -> Option<Vec<u8>> {
        match scope {
            Scope::Terminal(tid) => self.terminal.get(tid).and_then(|m| m.get(key)).cloned(),
            Scope::Collection(cid) => self.collection.get(cid).and_then(|m| m.get(key)).cloned(),
            Scope::Global => self.global.get(key).cloned(),
            // `Scope` is `#[non_exhaustive]`: a forward-compat variant we
            // don't know about returns None. The cleanest default for an
            // unknown scope is "no value present" — the caller's contract
            // is preserved without trapping on unknown bytes.
            _ => None,
        }
    }

    /// Set the value at `(scope, key)`. Returns whether the value
    /// actually changed (so the caller can suppress an unnecessary
    /// broadcast).
    pub fn set(&mut self, scope: &Scope, key: &str, value: Vec<u8>) -> MetadataSetOutcome {
        let bucket: &mut HashMap<String, Vec<u8>> = match scope {
            Scope::Terminal(tid) => self.terminal.entry(tid.clone()).or_default(),
            Scope::Collection(cid) => self.collection.entry(*cid).or_default(),
            Scope::Global => &mut self.global,
            // Unknown forward-compat variant: silently drop the write.
            // SPEC §6 lets newer encoders ship trailing field shapes;
            // here the surface area is "unknown scope, no bucket".
            _ => return MetadataSetOutcome::Unchanged,
        };
        if let Some(prev) = bucket.get(key)
            && prev == &value
        {
            return MetadataSetOutcome::Unchanged;
        }
        bucket.insert(key.to_owned(), value);
        MetadataSetOutcome::Changed
    }

    /// Delete `(scope, key)`. Returns whether the key existed (so the
    /// caller can suppress the broadcast on a true noop).
    pub fn delete(&mut self, scope: &Scope, key: &str) -> bool {
        match scope {
            Scope::Terminal(tid) => self
                .terminal
                .get_mut(tid)
                .and_then(|m| m.remove(key))
                .is_some(),
            Scope::Collection(cid) => self
                .collection
                .get_mut(cid)
                .and_then(|m| m.remove(key))
                .is_some(),
            Scope::Global => self.global.remove(key).is_some(),
            // Unknown forward-compat variant: nothing to delete.
            _ => false,
        }
    }

    /// List every key in `scope` (no values, sorted for determinism).
    #[must_use]
    pub fn list(&self, scope: &Scope) -> Vec<String> {
        let mut keys: Vec<String> = match scope {
            Scope::Terminal(tid) => self
                .terminal
                .get(tid)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            Scope::Collection(cid) => self
                .collection
                .get(cid)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
            Scope::Global => self.global.keys().cloned().collect(),
            // Unknown forward-compat variant: empty listing.
            _ => Vec::new(),
        };
        keys.sort();
        keys
    }

    /// Drop every key scoped to `terminal`. Called when the Terminal
    /// closes (the L1 lifecycle that owns the per-Terminal scope — see
    /// the `terminal` field doc). Subscriptions targeting the dead
    /// Terminal are connection-scoped and are reaped on detach, so they
    /// are left untouched here.
    pub fn forget_terminal(&mut self, terminal: &WireTerminalId) {
        self.terminal.remove(terminal);
    }

    /// Register `(client, scope, key)` as an active subscription. The
    /// underlying set is idempotent: re-subscribing the same triple is
    /// a noop.
    pub fn subscribe(&mut self, client: ClientId, scope: Scope, key: String) {
        self.subscriptions.insert((client, scope, key));
    }

    /// Drop every subscription owned by `client`. Called on detach.
    pub fn drop_client(&mut self, client: ClientId) {
        self.subscriptions.retain(|(c, _, _)| *c != client);
    }

    /// Collect every client subscribed to `(scope, key)`. Order is
    /// unspecified — callers MUST NOT rely on subscriber iteration order.
    #[must_use]
    pub fn subscribers_for(&self, scope: &Scope, key: &str) -> Vec<ClientId> {
        self.subscriptions
            .iter()
            .filter(|(_, s, k)| s == scope && k == key)
            .map(|(c, _, _)| *c)
            .collect()
    }
}
