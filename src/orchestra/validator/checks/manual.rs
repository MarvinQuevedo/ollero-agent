use crate::orchestra::types::CheckOutcome;

/// Placeholder for a human review gate.
/// Always returns Soft(0.5) — the orchestrator driver upgrades this
/// to Pass or Fail after collecting a real human decision at runtime.
pub fn manual_review(_note: &str) -> CheckOutcome {
    CheckOutcome::Soft(0.5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manual_review_is_soft_half() {
        match manual_review("please check the output") {
            CheckOutcome::Soft(s) => assert!((s - 0.5).abs() < f32::EPSILON),
            other => panic!("expected Soft(0.5), got {:?}", other),
        }
    }

    #[test]
    fn test_manual_review_empty_note() {
        match manual_review("") {
            CheckOutcome::Soft(s) => assert!((s - 0.5).abs() < f32::EPSILON),
            other => panic!("expected Soft(0.5), got {:?}", other),
        }
    }
}
