use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use anyhow::{Context, Result, anyhow};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use comrak::plugins::syntect::{SyntectAdapter, SyntectAdapterBuilder};
use url::Url;

use crate::{cli::Theme, core::terminal_palette::TerminalPalette};

static LIGHT_SYNTAX_ADAPTER: OnceLock<SyntectAdapter> = OnceLock::new();
static DARK_SYNTAX_ADAPTER: OnceLock<SyntectAdapter> = OnceLock::new();

pub(super) fn theme_styles(theme: Theme, palette: Option<TerminalPalette>) -> String {
    let font_faces = font_face_css();
    let styles = match theme {
        Theme::Light | Theme::System => [
            font_faces.as_str(),
            "\n",
            LIGHT_CSS,
            "\n",
            COMMON_OVERRIDES,
            "\n",
            LIGHT_THEME_OVERRIDES,
        ]
        .concat(),
        Theme::Dark => [
            font_faces.as_str(),
            "\n",
            DARK_CSS,
            "\n",
            COMMON_OVERRIDES,
            "\n",
            DARK_THEME_OVERRIDES,
        ]
        .concat(),
    };
    let styles = [styles.as_str(), "\n", FRONT_MATTER_STYLES].concat();
    match palette {
        Some(palette) => recolor(&styles, theme, palette),
        None => styles,
    }
}

fn recolor(styles: &str, theme: Theme, palette: TerminalPalette) -> String {
    let roles: &[(&[&str], String)] = match theme {
        Theme::Dark => &[
            (&["#f0f6fc"], palette.foreground.hex()),
            (&["#0d1117"], palette.background.hex()),
            (&["#9198a1", "#8b949e"], palette.muted().hex()),
            (&["#3d444d", "#30363d"], palette.border().hex()),
            (&["#151b23", "#161b22"], palette.surface().hex()),
            (&["#4493f8"], palette.link().hex()),
            (&["#1f6feb", "#1158c7"], palette.accent().hex()),
            (&["#656c76"], palette.inline_code().hex()),
        ],
        Theme::Light | Theme::System => &[
            (&["#1f2328"], palette.foreground.hex()),
            (&["#ffffff"], palette.background.hex()),
            (&["#59636e", "#57606a"], palette.muted().hex()),
            (&["#d1d9e0", "#d0d7de"], palette.border().hex()),
            (&["#f6f8fa"], palette.surface().hex()),
            (&["#0969da"], palette.link().hex()),
        ],
    };
    roles.iter().fold(styles.to_owned(), |styles, (originals, replacement)| {
        originals.iter().fold(styles, |styles, original| {
            styles.replace(original, replacement).replace(&original.to_uppercase(), replacement)
        })
    })
}

const FRONT_MATTER_STYLES: &str = r#"
.markdown-body table.mdv-front-matter {
  width: auto;
  margin-bottom: 24px;
}
.markdown-body table.mdv-front-matter th {
  text-align: left;
  font-weight: 600;
  white-space: nowrap;
}
"#;

pub(super) fn syntax_adapter(theme: Theme) -> &'static SyntectAdapter {
    match theme {
        Theme::Light | Theme::System => LIGHT_SYNTAX_ADAPTER
            .get_or_init(|| SyntectAdapterBuilder::new().theme("InspiredGitHub").build()),
        Theme::Dark => DARK_SYNTAX_ADAPTER
            .get_or_init(|| SyntectAdapterBuilder::new().theme("base16-ocean.dark").build()),
    }
}

fn font_face_css() -> String {
    let mona_sans_url = font_data_url(include_bytes!("../assets/fonts/MonaSansVF.woff2"));

    format!(
        r#"
@font-face {{
  font-family: 'Mona Sans VF';
  src: url('{mona_sans_url}') format('woff2');
  font-weight: 200 900;
  font-stretch: 75% 125%;
  font-style: normal;
  font-optical-sizing: auto;
}}

@font-face {{
  font-family: 'Mona Sans VF';
  src: url('{mona_sans_url}') format('woff2');
  font-weight: 200 900;
  font-stretch: 75% 125%;
  font-style: italic;
  font-optical-sizing: auto;
}}
"#
    )
}

fn font_data_url(bytes: &[u8]) -> String {
    format!("data:font/woff2;base64,{}", STANDARD.encode(bytes))
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if let Ok(path) = path.canonicalize() {
        return Ok(path);
    }

    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }

    Ok(std::env::current_dir()
        .context("failed to resolve the current directory for the HTML asset base URL")?
        .join(path))
}

pub(super) fn directory_url(path: &Path) -> Result<String> {
    let absolute_path = absolute_path(path)?;
    Url::from_directory_path(&absolute_path)
        .map(String::from)
        .map_err(|()| anyhow!("failed to create a file URL for {}", absolute_path.display()))
}

const LIGHT_CSS: &str = include_str!("../assets/github-markdown-light.css");

const COMMON_OVERRIDES: &str = r#"
body {
  margin: 0;
}
.mdv-page {
  padding: 8px;
  display: flex;
  justify-content: center;
}
.markdown-body {
  width: calc(100vw - 16px);
  max-width: none;
  background: transparent;
  font-family: 'Mona Sans VF', -apple-system, system-ui, "Segoe UI", "Noto Sans",
    Helvetica, Arial, sans-serif, "Apple Color Emoji", "Segoe UI Emoji";
}
.markdown-body img {
  max-width: 100%;
}
.markdown-body img:not([height]) {
  height: auto;
}
.markdown-body [align="center"] {
  text-align: center;
}
.markdown-body [align="left"] {
  text-align: left;
}
.markdown-body [align="right"] {
  text-align: right;
}
.markdown-body [align] > img:only-child {
  display: block;
}
.markdown-body [align="center"] > img:only-child {
  margin-left: auto;
  margin-right: auto;
}
.markdown-body [align="right"] > img:only-child {
  margin-left: auto;
}
.markdown-body [align="left"] > img:only-child {
  margin-right: auto;
}
.markdown-body span[data-math-style="inline"] {
  white-space: nowrap;
}
.markdown-body span[data-math-style="display"] {
  display: block;
  margin: 16px 0;
  text-align: center;
  white-space: pre-wrap;
}
.markdown-body p > img:only-child {
  display: block;
}
.mdv-mermaid {
  margin: 16px 0;
  display: flex;
  justify-content: center;
  overflow: hidden;
  max-width: min(100%, 420px);
  margin-left: auto;
  margin-right: auto;
}
.mdv-mermaid img,
.mdv-mermaid-diagram {
  display: block;
  width: 100%;
  max-width: 100%;
  height: auto;
  margin: 0 auto;
  flex: 0 1 auto;
}
.mdv-mermaid-fallback {
  color: #57606a;
  font-weight: 600;
}
.markdown-body code,
.markdown-body tt,
.markdown-body pre,
.markdown-body kbd,
.markdown-body samp {
  font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono",
    monospace !important;
}
"#;

const LIGHT_THEME_OVERRIDES: &str = r#"
body {
  background: #ffffff;
}
.markdown-body .highlight,
.markdown-body pre {
  background-color: #f6f8fa !important;
  border: 1px solid #d0d7de;
  border-radius: 6px;
}
.markdown-body .highlight pre,
.markdown-body pre {
  padding: 16px;
}
.markdown-body pre code,
.markdown-body .highlight pre code {
  background: transparent !important;
}
"#;

const DARK_CSS: &str = include_str!("../assets/github-markdown-dark.css");

const DARK_THEME_OVERRIDES: &str = r#"
body {
  background: #0d1117;
}
.mdv-mermaid-fallback {
  color: #8b949e;
  font-weight: 600;
}
.markdown-body .highlight,
.markdown-body pre {
  background-color: #161b22 !important;
  border: 1px solid #30363d;
  border-radius: 6px;
}
.markdown-body .highlight pre,
.markdown-body pre {
  padding: 16px;
}
.markdown-body pre code,
.markdown-body .highlight pre code {
  background: transparent !important;
}
"#;
