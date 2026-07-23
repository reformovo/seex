use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

const REFRESH: &[u8] = br##"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16" fill="none"><path stroke="#000" stroke-linecap="round" stroke-width="1.2" d="M4.667 14V8m0 0H2m2.667 0h2.666"/><path stroke="#000" stroke-linecap="round" stroke-linejoin="round" stroke-width="1.2" d="M14 12.667h-3.333V9.333"/><path fill="#000" d="M4.03 3.5a.667.667 0 0 0 .883 1l-.882-1ZM8 3.333A4.667 4.667 0 0 1 12.666 8H14a6 6 0 0 0-6-6v1.333ZM12.666 8a4.656 4.656 0 0 1-1.75 3.643l.834 1.04A5.99 5.99 0 0 0 14 8h-1.334ZM5.667 3.957A4.642 4.642 0 0 1 8 3.333V2a5.976 5.976 0 0 0-3 .803l.667 1.154Zm-.754.543c.232-.205.485-.387.754-.543l-.668-1.154c-.346.2-.67.434-.968.697l.882 1Z"/></svg>"##;
const PLUS: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M3.33325 8H12.6666" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/><path d="M8 3.33333V12.6667" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CLOSE: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M4.70581 4.5L11.294 11.5M11.294 4.5L4.70581 11.5" stroke="black" stroke-width="1.2" stroke-linecap="round"/></svg>"##;
const CHEVRON_RIGHT: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M6.44653 4.16071L10.1608 8.00143L6.44653 11.8571" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const CHEVRON_DOWN: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M4.15186 6.47321L7.99258 10.1696L11.8483 6.47321" stroke="black" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##;
const ELLIPSIS: &[u8] = br##"<svg width="16" height="16" viewBox="0 0 16 16" fill="none" xmlns="http://www.w3.org/2000/svg"><circle cx="3" cy="8" r="1" fill="black" stroke="black" stroke-width="0.5"/><circle cx="8" cy="8" r="1" fill="black" stroke="black" stroke-width="0.5"/><circle cx="13" cy="8" r="1" fill="black" stroke="black" stroke-width="0.5"/></svg>"##;

const ASSETS: [(&str, &[u8]); 6] = [
    ("icons/refresh.svg", REFRESH),
    ("icons/plus.svg", PLUS),
    ("icons/close.svg", CLOSE),
    ("icons/chevron-right.svg", CHEVRON_RIGHT),
    ("icons/chevron-down.svg", CHEVRON_DOWN),
    ("icons/ellipsis.svg", ELLIPSIS),
];

pub struct ViewerAssets;

impl AssetSource for ViewerAssets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS
            .iter()
            .find_map(|(asset_path, bytes)| (*asset_path == path).then_some(Cow::Borrowed(*bytes))))
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        let prefix = path.trim_end_matches('/');
        Ok(ASSETS
            .iter()
            .filter_map(|(asset_path, _)| {
                asset_path
                    .strip_prefix(prefix)
                    .and_then(|suffix| suffix.strip_prefix('/'))
                    .filter(|suffix| !suffix.contains('/'))
                    .map(SharedString::from)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_paths_are_loadable_and_listed() -> gpui::Result<()> {
        let assets = ViewerAssets;

        for (path, _) in ASSETS {
            assert!(
                assets.load(path)?.is_some(),
                "missing embedded asset {path}"
            );
        }
        assert_eq!(assets.list("icons")?.len(), ASSETS.len());
        assert!(assets.load("icons/missing.svg")?.is_none());
        Ok(())
    }
}
