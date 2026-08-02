//! Compatibility re-exports for the storage implementation now owned by `seex`.

#![forbid(unsafe_code)]

pub use seex::storage::*;

#[cfg(test)]
mod tests {
    #[test]
    fn legacy_path_reexports_the_migrated_type() {
        fn round_trip(value: seex::storage::ProjectConnection) -> super::ProjectConnection {
            value
        }

        let _same_type = round_trip;
    }
}
