//! Pod event decoder.
//!
//! The `sla-escrow` program emits Pod-encoded events via `sol_log_data`
//! (Solana's `Program data:` log line). Each emitted event is a single
//! base64-encoded byte slice whose length is fixed by the underlying
//! `#[repr(C)]` struct in `sla_escrow_api::event`.
//!
//! We discriminate **by exact byte length**: every payment-lifecycle event we
//! care about has a unique size, so length+bytemuck is enough. This avoids
//! prefixing or guessing.

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use serde::Serialize;
use sla_escrow_api::event::{
    DeliverySubmittedEvent, PaymentClosedEvent, PaymentCreatedWithFundEvent, PaymentExpiredEvent,
    PaymentFundedEvent, PaymentOracleConfirmedEvent, PaymentRefundedEvent, PaymentReleasedEvent,
    PaymentTTLExtendedEvent,
};

const PROGRAM_DATA_PREFIX: &str = "Program data: ";

/// A decoded reputation-relevant event.
///
/// Only the six events that feed the scorecard are surfaced here. Operational
/// events (Bank, Authority, Config) are not decoded — they don't affect oracle
/// reputation. If a future increment needs them, add a variant + size match.
#[derive(Debug, Clone)]
pub enum DecodedEvent {
    PaymentCreated(PaymentCreatedWithFundEvent),
    DeliverySubmitted(DeliverySubmittedEvent),
    PaymentOracleConfirmed(PaymentOracleConfirmedEvent),
    PaymentReleased(PaymentReleasedEvent),
    PaymentRefunded(PaymentRefundedEvent),
    PaymentTTLExtended(PaymentTTLExtendedEvent),
    PaymentClosed(PaymentClosedEvent),
    PaymentExpired(PaymentExpiredEvent),
    PaymentFunded(PaymentFundedEvent),
}

impl DecodedEvent {
    /// Stable string tag used as the `oracle_events.event_type` column.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::PaymentCreated(_) => "payment_created",
            Self::DeliverySubmitted(_) => "delivery_submitted",
            Self::PaymentOracleConfirmed(_) => "payment_oracle_confirmed",
            Self::PaymentReleased(_) => "payment_released",
            Self::PaymentRefunded(_) => "payment_refunded",
            Self::PaymentTTLExtended(_) => "payment_ttl_extended",
            Self::PaymentClosed(_) => "payment_closed",
            Self::PaymentExpired(_) => "payment_expired",
            Self::PaymentFunded(_) => "payment_funded",
        }
    }

    /// 32-byte payment_uid the event refers to. Every reputation-relevant
    /// event carries one.
    pub fn payment_uid(&self) -> [u8; 32] {
        match self {
            Self::PaymentCreated(e) => e.payment_uid,
            Self::DeliverySubmitted(e) => e.payment_uid,
            Self::PaymentOracleConfirmed(e) => e.payment_uid,
            Self::PaymentReleased(e) => e.payment_uid,
            Self::PaymentRefunded(e) => e.payment_uid,
            Self::PaymentTTLExtended(e) => e.payment_uid,
            Self::PaymentClosed(e) => e.payment_uid,
            Self::PaymentExpired(e) => e.payment_uid,
            Self::PaymentFunded(e) => e.payment_uid,
        }
    }

    /// JSON projection of the event, suitable for the `oracle_events.payload`
    /// column. Field names are camelCase to match the wider x402 wire
    /// convention.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            Self::PaymentCreated(e) => serde_json::json!(PaymentCreatedJson::from(e)),
            Self::DeliverySubmitted(e) => serde_json::json!(DeliverySubmittedJson::from(e)),
            Self::PaymentOracleConfirmed(e) => {
                serde_json::json!(PaymentOracleConfirmedJson::from(e))
            }
            Self::PaymentReleased(e) => serde_json::json!(PaymentReleasedJson::from(e)),
            Self::PaymentRefunded(e) => serde_json::json!(PaymentRefundedJson::from(e)),
            Self::PaymentTTLExtended(e) => serde_json::json!(PaymentTTLExtendedJson::from(e)),
            Self::PaymentClosed(e) => serde_json::json!(PaymentClosedJson::from(e)),
            Self::PaymentExpired(e) => serde_json::json!(PaymentExpiredJson::from(e)),
            Self::PaymentFunded(e) => serde_json::json!(PaymentFundedJson::from(e)),
        }
    }
}

/// Decode every `Program data:` line in `logs` into a [`DecodedEvent`]. Lines
/// that don't decode to one of our known sizes are silently ignored — the
/// program emits operational events too (Bank/Authority/Config) and we don't
/// surface those in Increment 1.
///
/// Returns `(line_index, event)` pairs so the writer can persist `log_index`
/// (the position within the tx's `meta.logMessages` array). This makes the
/// `(signature, log_index)` primary key stable on replay.
pub fn decode_program_data_lines(logs: &[String]) -> Vec<(usize, DecodedEvent)> {
    let mut out = Vec::new();
    for (idx, line) in logs.iter().enumerate() {
        let line = line.trim();
        let Some(b64) = line.strip_prefix(PROGRAM_DATA_PREFIX) else {
            continue;
        };
        let Ok(bytes) = B64.decode(b64.trim()) else {
            continue;
        };
        if let Some(ev) = decode_event_bytes(&bytes) {
            out.push((idx, ev));
        }
    }
    out
}

/// Inner decoder, exposed for unit tests. Returns `None` on size mismatch or
/// bytemuck failure (event family we don't care about, or a corrupt log).
pub fn decode_event_bytes(bytes: &[u8]) -> Option<DecodedEvent> {
    match bytes.len() {
        n if n == std::mem::size_of::<PaymentCreatedWithFundEvent>() => {
            bytemuck::try_from_bytes::<PaymentCreatedWithFundEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentCreated(*e))
        }
        n if n == std::mem::size_of::<DeliverySubmittedEvent>() => {
            bytemuck::try_from_bytes::<DeliverySubmittedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::DeliverySubmitted(*e))
        }
        n if n == std::mem::size_of::<PaymentOracleConfirmedEvent>() => {
            bytemuck::try_from_bytes::<PaymentOracleConfirmedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentOracleConfirmed(*e))
        }
        n if n == std::mem::size_of::<PaymentReleasedEvent>() => {
            bytemuck::try_from_bytes::<PaymentReleasedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentReleased(*e))
        }
        n if n == std::mem::size_of::<PaymentRefundedEvent>() => {
            bytemuck::try_from_bytes::<PaymentRefundedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentRefunded(*e))
        }
        n if n == std::mem::size_of::<PaymentTTLExtendedEvent>() => {
            bytemuck::try_from_bytes::<PaymentTTLExtendedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentTTLExtended(*e))
        }
        n if n == std::mem::size_of::<PaymentClosedEvent>() => {
            bytemuck::try_from_bytes::<PaymentClosedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentClosed(*e))
        }
        n if n == std::mem::size_of::<PaymentExpiredEvent>() => {
            bytemuck::try_from_bytes::<PaymentExpiredEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentExpired(*e))
        }
        n if n == std::mem::size_of::<PaymentFundedEvent>() => {
            bytemuck::try_from_bytes::<PaymentFundedEvent>(bytes)
                .ok()
                .map(|e| DecodedEvent::PaymentFunded(*e))
        }
        _ => None,
    }
}

// ────────────────────────── JSON projections ────────────────────────────────

#[derive(Serialize)]
struct PaymentCreatedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    #[serde(rename = "slaHash")]
    sla_hash: String,
    escrow: String,
    buyer: String,
    seller: String,
    mint: String,
    #[serde(rename = "oracleAuthority")]
    oracle_authority: String,
    amount: u64,
    #[serde(rename = "expiresAt")]
    expires_at: i64,
    timestamp: i64,
    state: u8,
}
impl From<&PaymentCreatedWithFundEvent> for PaymentCreatedJson {
    fn from(e: &PaymentCreatedWithFundEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            sla_hash: hex::encode(e.sla_hash),
            escrow: e.escrow.to_string(),
            buyer: e.buyer.to_string(),
            seller: e.seller.to_string(),
            mint: e.mint.to_string(),
            oracle_authority: e.oracle_authority.to_string(),
            amount: e.amount,
            expires_at: e.expires_at,
            timestamp: e.timestamp,
            state: e.state,
        }
    }
}

#[derive(Serialize)]
struct DeliverySubmittedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    #[serde(rename = "deliveryHash")]
    delivery_hash: String,
    timestamp: i64,
    seller: String,
}
impl From<&DeliverySubmittedEvent> for DeliverySubmittedJson {
    fn from(e: &DeliverySubmittedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            delivery_hash: hex::encode(e.delivery_hash),
            timestamp: e.timestamp,
            seller: e.seller.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PaymentOracleConfirmedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    #[serde(rename = "oracleAuthority")]
    oracle_authority: String,
    #[serde(rename = "deliveryHash")]
    delivery_hash: String,
    #[serde(rename = "resolutionHash")]
    resolution_hash: String,
    #[serde(rename = "slaHash")]
    sla_hash: String,
    amount: u64,
    timestamp: i64,
    #[serde(rename = "resolutionReason")]
    resolution_reason: u16,
    #[serde(rename = "resolutionState")]
    resolution_state: u8,
}
impl From<&PaymentOracleConfirmedEvent> for PaymentOracleConfirmedJson {
    fn from(e: &PaymentOracleConfirmedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            oracle_authority: e.oracle_authority.to_string(),
            delivery_hash: hex::encode(e.delivery_hash),
            resolution_hash: hex::encode(e.resolution_hash),
            sla_hash: hex::encode(e.sla_hash),
            amount: e.amount,
            timestamp: e.timestamp,
            resolution_reason: e.resolution_reason,
            resolution_state: e.resolution_state,
        }
    }
}

#[derive(Serialize)]
struct PaymentReleasedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    mint: String,
    amount: u64,
    #[serde(rename = "oracleTip")]
    oracle_tip: u64,
    timestamp: i64,
    seller: String,
    #[serde(rename = "isExpired")]
    is_expired: u8,
}
impl From<&PaymentReleasedEvent> for PaymentReleasedJson {
    fn from(e: &PaymentReleasedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            mint: e.mint.to_string(),
            amount: e.amount,
            oracle_tip: e.oracle_tip,
            timestamp: e.timestamp,
            seller: e.seller.to_string(),
            is_expired: e.is_expired,
        }
    }
}

#[derive(Serialize)]
struct PaymentRefundedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    mint: String,
    amount: u64,
    #[serde(rename = "oracleTip")]
    oracle_tip: u64,
    timestamp: i64,
    buyer: String,
}
impl From<&PaymentRefundedEvent> for PaymentRefundedJson {
    fn from(e: &PaymentRefundedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            mint: e.mint.to_string(),
            amount: e.amount,
            oracle_tip: e.oracle_tip,
            timestamp: e.timestamp,
            buyer: e.buyer.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PaymentTTLExtendedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    #[serde(rename = "additionalSeconds")]
    additional_seconds: i64,
    #[serde(rename = "newExpiresAt")]
    new_expires_at: i64,
    timestamp: i64,
    buyer: String,
}
impl From<&PaymentTTLExtendedEvent> for PaymentTTLExtendedJson {
    fn from(e: &PaymentTTLExtendedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            additional_seconds: e.additional_seconds,
            new_expires_at: e.new_expires_at,
            timestamp: e.timestamp,
            buyer: e.buyer.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PaymentClosedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    timestamp: i64,
    closer: String,
}
impl From<&PaymentClosedEvent> for PaymentClosedJson {
    fn from(e: &PaymentClosedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            timestamp: e.timestamp,
            closer: e.closer.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PaymentExpiredJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    mint: String,
    amount: u64,
    timestamp: i64,
    buyer: String,
    seller: String,
}
impl From<&PaymentExpiredEvent> for PaymentExpiredJson {
    fn from(e: &PaymentExpiredEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            mint: e.mint.to_string(),
            amount: e.amount,
            timestamp: e.timestamp,
            buyer: e.buyer.to_string(),
            seller: e.seller.to_string(),
        }
    }
}

#[derive(Serialize)]
struct PaymentFundedJson {
    #[serde(rename = "paymentUid")]
    payment_uid: String,
    mint: String,
    amount: u64,
    timestamp: i64,
    buyer: String,
}
impl From<&PaymentFundedEvent> for PaymentFundedJson {
    fn from(e: &PaymentFundedEvent) -> Self {
        Self {
            payment_uid: hex::encode(e.payment_uid),
            mint: e.mint.to_string(),
            amount: e.amount,
            timestamp: e.timestamp,
            buyer: e.buyer.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use solana_sdk::pubkey::Pubkey;

    use super::*;

    /// Helper: encode an event the same way the program does (`sol_log_data`
    /// → `Program data: <base64>`) and then verify the decoder round-trips.
    fn round_trip<T: bytemuck::Pod>(ev: &T) -> Option<DecodedEvent> {
        let raw = bytemuck::bytes_of(ev);
        let line = format!("{}{}", PROGRAM_DATA_PREFIX, B64.encode(raw));
        decode_program_data_lines(&[line])
            .into_iter()
            .next()
            .map(|(_, e)| e)
    }

    #[test]
    fn payment_created_round_trip() {
        let ev = PaymentCreatedWithFundEvent {
            payment_uid: [1u8; 32],
            sla_hash: [2u8; 32],
            escrow: Pubkey::new_unique(),
            buyer: Pubkey::new_unique(),
            seller: Pubkey::new_unique(),
            mint: Pubkey::new_unique(),
            oracle_authority: Pubkey::new_unique(),
            amount: 1_000_000,
            expires_at: 1_900_000_000,
            timestamp: 1_800_000_000,
            state: 0,
            _padding: [0; 7],
        };
        let decoded = round_trip(&ev).expect("decode");
        match decoded {
            DecodedEvent::PaymentCreated(d) => {
                assert_eq!(d.payment_uid, ev.payment_uid);
                assert_eq!(d.amount, 1_000_000);
                assert_eq!(d.expires_at, 1_900_000_000);
            }
            other => panic!("expected PaymentCreated, got {:?}", other.event_type()),
        }
    }

    #[test]
    fn delivery_submitted_round_trip() {
        let ev = DeliverySubmittedEvent {
            payment_uid: [3u8; 32],
            delivery_hash: [4u8; 32],
            timestamp: 1_800_000_010,
            seller: Pubkey::new_unique(),
        };
        let decoded = round_trip(&ev).expect("decode");
        assert_eq!(decoded.event_type(), "delivery_submitted");
        assert_eq!(decoded.payment_uid(), [3u8; 32]);
    }

    #[test]
    fn payment_oracle_confirmed_round_trip_carries_oracle_authority() {
        let oracle = Pubkey::new_unique();
        let ev = PaymentOracleConfirmedEvent {
            payment_uid: [5u8; 32],
            oracle_authority: oracle,
            delivery_hash: [6u8; 32],
            resolution_hash: [7u8; 32],
            sla_hash: [8u8; 32],
            amount: 500_000,
            timestamp: 1_800_000_020,
            resolution_reason: 200, // economic refusal
            resolution_state: 2,
            _padding: [0; 5],
        };
        let decoded = round_trip(&ev).expect("decode");
        match decoded {
            DecodedEvent::PaymentOracleConfirmed(d) => {
                assert_eq!(d.oracle_authority, oracle);
                assert_eq!(d.resolution_reason, 200);
                assert_eq!(d.resolution_state, 2);
            }
            other => panic!(
                "expected PaymentOracleConfirmed, got {:?}",
                other.event_type()
            ),
        }
    }

    #[test]
    fn payment_released_carries_oracle_tip() {
        let ev = PaymentReleasedEvent {
            payment_uid: [9u8; 32],
            mint: Pubkey::new_unique(),
            amount: 1_000_000,
            oracle_tip: 10_000,
            timestamp: 1_800_000_030,
            seller: Pubkey::new_unique(),
            is_expired: 0,
            _padding: [0; 7],
        };
        let decoded = round_trip(&ev).expect("decode");
        match decoded {
            DecodedEvent::PaymentReleased(d) => {
                assert_eq!(d.oracle_tip, 10_000);
                assert_eq!(d.amount, 1_000_000);
            }
            other => panic!("expected PaymentReleased, got {:?}", other.event_type()),
        }
    }

    #[test]
    fn payment_refunded_round_trip() {
        let ev = PaymentRefundedEvent {
            payment_uid: [10u8; 32],
            mint: Pubkey::new_unique(),
            amount: 500_000,
            oracle_tip: 5_000,
            timestamp: 1_800_000_040,
            buyer: Pubkey::new_unique(),
        };
        let decoded = round_trip(&ev).expect("decode");
        match decoded {
            DecodedEvent::PaymentRefunded(d) => {
                assert_eq!(d.oracle_tip, 5_000);
            }
            other => panic!("expected PaymentRefunded, got {:?}", other.event_type()),
        }
    }

    #[test]
    fn unknown_size_is_silently_ignored() {
        // Bank / Authority / Config events have other sizes — the decoder
        // returns None and the writer skips them.
        let bytes = vec![0u8; 64]; // doesn't match any payment-lifecycle event
        assert!(decode_event_bytes(&bytes).is_none());
    }

    #[test]
    fn malformed_log_lines_are_silently_ignored() {
        let lines = vec![
            "Program log: random message".to_string(),
            "Program data: not-base64-!!!".to_string(),
            "Program data: aGVsbG8=".to_string(), // valid base64 but wrong size
            "completely unrelated log".to_string(),
        ];
        let decoded = decode_program_data_lines(&lines);
        assert!(decoded.is_empty());
    }

    #[test]
    fn json_projection_uses_camel_case_and_hex() {
        let ev = DeliverySubmittedEvent {
            payment_uid: [0xAB; 32],
            delivery_hash: [0xCD; 32],
            timestamp: 42,
            seller: Pubkey::new_unique(),
        };
        let decoded = DecodedEvent::DeliverySubmitted(ev);
        let json = decoded.to_json();
        assert_eq!(
            json["paymentUid"].as_str().unwrap(),
            "abababababababababababababababababababababababababababababababab"
        );
        assert_eq!(
            json["deliveryHash"].as_str().unwrap(),
            "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd"
        );
        assert_eq!(json["timestamp"].as_i64().unwrap(), 42);
    }

    #[test]
    fn line_index_preserves_log_position() {
        let ev1 = DeliverySubmittedEvent {
            payment_uid: [1u8; 32],
            delivery_hash: [2u8; 32],
            timestamp: 1,
            seller: Pubkey::new_unique(),
        };
        let ev2 = PaymentClosedEvent {
            payment_uid: [3u8; 32],
            timestamp: 2,
            closer: Pubkey::new_unique(),
        };
        let lines = vec![
            "Program log: header".to_string(),
            format!(
                "{}{}",
                PROGRAM_DATA_PREFIX,
                B64.encode(bytemuck::bytes_of(&ev1))
            ),
            "Program log: middle".to_string(),
            format!(
                "{}{}",
                PROGRAM_DATA_PREFIX,
                B64.encode(bytemuck::bytes_of(&ev2))
            ),
        ];
        let decoded = decode_program_data_lines(&lines);
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].0, 1); // first event at log line 1
        assert_eq!(decoded[1].0, 3); // second event at log line 3
    }

    #[test]
    fn all_event_sizes_are_distinct() {
        // Defense against future event additions: if two payment-lifecycle
        // events ever shared a size, the size-discriminator decoder would
        // misclassify one. This test fails loudly if that ever happens.
        let sizes = [
            std::mem::size_of::<PaymentCreatedWithFundEvent>(),
            std::mem::size_of::<DeliverySubmittedEvent>(),
            std::mem::size_of::<PaymentOracleConfirmedEvent>(),
            std::mem::size_of::<PaymentReleasedEvent>(),
            std::mem::size_of::<PaymentRefundedEvent>(),
            std::mem::size_of::<PaymentTTLExtendedEvent>(),
            std::mem::size_of::<PaymentClosedEvent>(),
            std::mem::size_of::<PaymentExpiredEvent>(),
            std::mem::size_of::<PaymentFundedEvent>(),
        ];
        let unique: std::collections::HashSet<_> = sizes.iter().collect();
        assert_eq!(
            sizes.len(),
            unique.len(),
            "two payment-lifecycle event types share a Pod size; decoder needs a real discriminator"
        );
    }
}
