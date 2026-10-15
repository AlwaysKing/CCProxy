// 客户端配置目录：身份判定、扫描、检查、写入（claude 的 settings.json / codex 的 config.toml
// 与 models.json）。全部只依赖标准库，可脱离界面单测。

pub mod apply;
pub mod catalog;
pub mod check;
pub mod daemon;
pub mod dirs;
pub mod json;
pub mod toml;

pub use apply::{apply_client_config, sync_codex_catalogs};
pub use catalog::expected_catalog_entries;
pub use check::inspect_client_config;
pub use dirs::{detect_dir_kind, scan_client_dirs};

