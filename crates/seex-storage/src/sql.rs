pub(crate) fn string_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_literal_escapes_single_quotes() {
        assert_eq!(string_literal("canary's/data"), "'canary''s/data'");
    }
}
