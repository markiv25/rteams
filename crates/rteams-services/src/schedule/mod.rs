//! schedule: cron parsing and next-fire times (`cron`), the pure fire decision (`runner`),
//! run history files (`history`) and warm-up bookkeeping (`store`).

pub mod cron;
pub mod history;
pub mod runner;
pub mod store;

#[cfg(test)]
mod tests {
    #[test]
    fn schedule_serde_is_backward_compatible() {
        let old = r#"{"id":"s","member":"a","prompt":"p","every_minutes":5,"enabled":true}"#;
        let s: rteams_core::Schedule = serde_json::from_str(old).unwrap();
        assert!(s.cron.is_none() && s.timezone.is_none() && s.warmup_minutes.is_none());
        assert_eq!(serde_json::to_string(&s).unwrap(), old);
        let n = rteams_core::Schedule {
            cron: Some("0 9 * * *".into()),
            warmup_minutes: Some(5),
            ..s
        };
        let back: rteams_core::Schedule =
            serde_json::from_str(&serde_json::to_string(&n).unwrap()).unwrap();
        assert_eq!(back, n);
    }
}
