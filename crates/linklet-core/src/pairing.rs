//! Permanent pairing-code validation and the shared online attempt budget.
use std::time::Duration;

/// Requires exactly six ASCII digits, preserving leading zeros.
pub fn validate_code(code: &str) -> Result<(), String> {
    if code.len() == 6 && code.bytes().all(|b| b.is_ascii_digit()) {
        Ok(())
    } else {
        Err("code must be a string of exactly six digits".into())
    }
}

/// Agent-wide budget: five immediate attempts, then one per minute. All peers share it.
#[derive(Default)]
pub struct PairingBudget {
    next: Duration,
}
impl PairingBudget {
    /// Consumes an attempt, or returns the remaining cooldown. Time is supplied by the caller.
    pub fn attempt(&mut self, now: Duration) -> Result<(), Duration> {
        let allowed = now.saturating_add(Duration::from_secs(240));
        if allowed < self.next {
            return Err(self.next - allowed);
        }
        self.next = self.next.max(now).saturating_add(Duration::from_secs(60));
        Ok(())
    }
}

/// Encodes one OPAQUE handshake message; command-channel semantics are unchanged.
pub fn message(bytes: &[u8]) -> serde_json::Value {
    serde_json::json!({"pairing":"opaque-v1","protocol":crate::wire::PROTOCOL_VERSION,"message":crate::wire::to_hex(bytes)})
}
/// Validates the pairing scheme, protocol and bounded hex payload before cryptography.
pub fn read_message(value: &serde_json::Value) -> Result<Vec<u8>, String> {
    if value["pairing"] != "opaque-v1" {
        return Err("unsupported pairing protocol; update both ends".into());
    }
    if value["protocol"] != crate::wire::PROTOCOL_VERSION {
        return Err("protocol mismatch; update both ends".into());
    }
    let text = value["message"].as_str().ok_or("missing pairing message")?;
    if text.len() > 2048 {
        return Err("pairing message is too large".into());
    }
    crate::wire::from_hex(text).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn five_attempts_burst_then_one_per_minute() {
        let mut budget = PairingBudget::default();
        for _ in 0..5 {
            assert!(budget.attempt(Duration::ZERO).is_ok());
        }
        assert_eq!(
            budget.attempt(Duration::from_secs(1)),
            Err(Duration::from_secs(59))
        );
        assert!(budget.attempt(Duration::from_secs(60)).is_ok());
        assert_eq!(
            budget.attempt(Duration::from_secs(60)),
            Err(Duration::from_secs(60))
        );
    }
    #[test]
    fn code_preserves_zeroes_and_rejects_other_shapes() {
        assert!(validate_code("000017").is_ok());
        for code in ["12345", "1234567", " 12345", "abcdef", ""] {
            assert!(validate_code(code).is_err());
        }
    }
}
