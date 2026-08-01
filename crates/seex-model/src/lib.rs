//! Compatibility re-exports for product types now owned by `seex`.

#![forbid(unsafe_code)]

pub use seex::model::*;

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_path_reexports_the_facade_type() {
        fn round_trip(value: seex::ProjectId) -> super::types::ProjectId {
            value
        }

        let _same_type = round_trip;
    }
}
