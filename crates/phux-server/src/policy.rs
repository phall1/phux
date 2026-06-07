//! Server-side policy engine traits and default permissive implementations.
//!
//! This module defines extension points that downstream consumers can
//! implement to enforce authorization, audit logging, and input provenance.
//! The default implementations are permissive (allow everything, log
//! nothing) so a server without a custom policy engine behaves exactly
//! as before.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use phux_protocol::ids::{GroupId, TerminalId};
use phux_protocol::policy::{
    AuditEvent, Capability, ConsumerClass, ConsumerId, Decision, InputTag, MetadataOp,
    MetadataScope, PeerIdentity, TaggedInput, TerminalOp,
};
use tracing::trace;

/// Extension point for authorization decisions.
///
/// The server calls this trait at every security-relevant decision point.
/// Implementations may deny operations, attenuate capabilities, or flag
/// anomalies for downstream review.
///
/// All methods are `&self` so the implementation can be shared across
/// tasks (typically via `Arc<dyn PolicyEngine>`).
///
/// The trait is object-safe: every method returns a `Pin<Box<dyn Future>>`
/// so it can be used as a trait object.
pub trait PolicyEngine: Send + Sync {
    /// Authorize a HELLO handshake. Returns the capabilities that should
    /// be granted to this consumer. The server intersects the returned
    /// capabilities with what the consumer requested.
    fn authorize_hello<'a>(
        &'a self,
        peer_identity: &'a PeerIdentity,
        requested_caps: Vec<Capability>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Capability>, PolicyError>> + Send + 'a>>;

    /// Authorize a terminal operation.
    fn authorize_terminal_op<'a>(
        &'a self,
        consumer: &'a ConsumerId,
        terminal_id: &'a TerminalId,
        op: &'a TerminalOp,
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>>;

    /// Authorize a group operation.
    fn authorize_group_op<'a>(
        &'a self,
        consumer: &'a ConsumerId,
        group_id: &'a GroupId,
        op: &'a phux_protocol::policy::GroupOp,
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>>;

    /// Authorize a metadata operation.
    fn authorize_metadata_op<'a>(
        &'a self,
        consumer: &'a ConsumerId,
        scope: &'a MetadataScope,
        op: &'a MetadataOp,
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>>;

    /// Authorize a satellite routing operation (federation).
    fn authorize_satellite_route<'a>(
        &'a self,
        hub_consumer: &'a ConsumerId,
        satellite: &'a str,
        delegated_caps: &'a [Capability],
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>>;
}

/// A sink for durable audit events.
///
/// The server emits an `AuditEvent` at every policy decision point.
/// Implementations may write to a file, stream to a SIEM, or drop
/// events silently.
pub trait AuditSink: Send + Sync {
    /// Write a single audit event.
    fn write<'a>(
        &'a self,
        event: AuditEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuditError>> + Send + 'a>>;

    /// Query previously-written events. Optional: default impl returns empty.
    fn query<'a>(
        &'a self,
        filter: AuditFilter,
        limit: usize,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<AuditEvent>, AuditError>> + Send + 'a>> {
        Box::pin(async move {
            let _ = filter;
            let _ = limit;
            Ok(vec![])
        })
    }
}

/// Tag input frames with provenance metadata.
///
/// Called for every input frame before it is routed to the PTY.
/// Implementations may classify consumers (human vs agent) and attach
/// attestation chains.
pub trait InputProvenance: Send + Sync {
    /// Tag a raw input frame with provenance metadata.
    fn tag(&self, consumer: &ConsumerId, terminal_id: &TerminalId, payload: &[u8]) -> TaggedInput;

    /// Classify a consumer from its tag.
    fn classify(&self, tag: &InputTag) -> ConsumerClass {
        tag.class
    }
}

/// A policy engine that allows everything.
///
/// This is the default when no custom policy engine is configured.
/// It grants all requested capabilities and allows every operation.
#[derive(Debug, Clone, Copy)]
pub struct PermissivePolicy;

impl PermissivePolicy {
    /// Shared instance (stateless).
    pub const INSTANCE: Self = Self;
}

impl PolicyEngine for PermissivePolicy {
    fn authorize_hello<'a>(
        &'a self,
        _peer_identity: &'a PeerIdentity,
        requested_caps: Vec<Capability>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Capability>, PolicyError>> + Send + 'a>> {
        Box::pin(async move {
            trace!("PermissivePolicy: authorizing HELLO");
            Ok(requested_caps)
        })
    }

    fn authorize_terminal_op<'a>(
        &'a self,
        _consumer: &'a ConsumerId,
        _terminal_id: &'a TerminalId,
        op: &'a TerminalOp,
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>> {
        Box::pin(async move {
            trace!(?op, "PermissivePolicy: authorizing terminal op");
            Ok(Decision::Allow)
        })
    }

    fn authorize_group_op<'a>(
        &'a self,
        _consumer: &'a ConsumerId,
        _group_id: &'a GroupId,
        _op: &'a phux_protocol::policy::GroupOp,
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>> {
        Box::pin(async move { Ok(Decision::Allow) })
    }

    fn authorize_metadata_op<'a>(
        &'a self,
        _consumer: &'a ConsumerId,
        _scope: &'a MetadataScope,
        _op: &'a MetadataOp,
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>> {
        Box::pin(async move { Ok(Decision::Allow) })
    }

    fn authorize_satellite_route<'a>(
        &'a self,
        _hub_consumer: &'a ConsumerId,
        _satellite: &'a str,
        _delegated_caps: &'a [Capability],
    ) -> Pin<Box<dyn Future<Output = Result<Decision, PolicyError>> + Send + 'a>> {
        Box::pin(async move { Ok(Decision::Allow) })
    }
}

/// An audit sink that drops every event silently.
#[derive(Debug, Clone, Copy)]
pub struct NoopAuditSink;

impl AuditSink for NoopAuditSink {
    fn write<'a>(
        &'a self,
        _event: AuditEvent,
    ) -> Pin<Box<dyn Future<Output = Result<(), AuditError>> + Send + 'a>> {
        Box::pin(async move {
            trace!("NoopAuditSink: dropping event");
            Ok(())
        })
    }
}

/// An input provenance tracker that tags everything as unknown.
#[derive(Debug, Clone, Copy)]
pub struct UnknownProvenance;

impl InputProvenance for UnknownProvenance {
    fn tag(&self, consumer: &ConsumerId, terminal_id: &TerminalId, payload: &[u8]) -> TaggedInput {
        TaggedInput {
            terminal_id: terminal_id.clone(),
            payload: payload.to_vec(),
            tag: InputTag {
                consumer: consumer.clone(),
                class: ConsumerClass::Unknown,
                chain: vec![],
                timestamp: chrono::Utc::now(),
            },
        }
    }
}

/// Bundle of policy extension points held by the server.
///
/// Cloning is cheap (all fields are `Arc<dyn ...>`).
#[derive(Clone)]
pub struct PolicyBundle {
    /// Authorization engine consulted at every decision point.
    pub engine: Arc<dyn PolicyEngine>,
    /// Sink for durable audit events.
    pub audit: Arc<dyn AuditSink>,
    /// Provenance tagger applied to inbound input frames.
    pub provenance: Arc<dyn InputProvenance>,
}

impl std::fmt::Debug for PolicyBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolicyBundle")
            .field("engine", &"<dyn PolicyEngine>")
            .field("audit", &"<dyn AuditSink>")
            .field("provenance", &"<dyn InputProvenance>")
            .finish()
    }
}

impl Default for PolicyBundle {
    fn default() -> Self {
        Self {
            engine: Arc::new(PermissivePolicy::INSTANCE),
            audit: Arc::new(NoopAuditSink),
            provenance: Arc::new(UnknownProvenance),
        }
    }
}

/// Filter for querying audit events.
#[derive(Debug, Clone, Default)]
pub struct AuditFilter {
    /// Restrict to events from this consumer.
    pub consumer: Option<ConsumerId>,
    /// Restrict to events targeting this terminal.
    pub terminal_id: Option<TerminalId>,
    /// Restrict to events whose action matches this type tag.
    pub action_type: Option<String>,
    /// Lower bound (inclusive) on event timestamp.
    pub from: Option<chrono::DateTime<chrono::Utc>>,
    /// Upper bound (inclusive) on event timestamp.
    pub to: Option<chrono::DateTime<chrono::Utc>>,
    /// Restrict to events with this decision.
    pub decision: Option<Decision>,
}

/// Errors from policy operations.
#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    /// The consumer is not permitted to perform the operation.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// A presented capability token has expired.
    #[error("expired capability")]
    ExpiredCapability,
    /// A satellite routing request was rejected as invalid.
    #[error("invalid satellite route")]
    InvalidSatelliteRoute,
    /// An internal error occurred inside the policy engine.
    #[error("internal: {0}")]
    Internal(String),
}

/// Errors from audit operations.
#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    /// Writing the event to the sink failed.
    #[error("write failed: {0}")]
    WriteFailed(String),
    /// Querying the sink for events failed.
    #[error("query failed: {0}")]
    QueryFailed(String),
}
