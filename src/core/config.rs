use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::{
    cli::{ColorSource, MdvArgs, ScrollStyle, Theme},
    core::terminal_palette::TerminalPalette,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppConfig {
    pub path: PathBuf,
    pub watch: bool,
    pub theme: Theme,
    pub mermaid_mode: MermaidMode,
    pub scroll: ScrollStyle,
    pub zoom_percent: u16,
    pub colors: ColorSource,
    pub palette: Option<TerminalPalette>,
}

impl TryFrom<MdvArgs> for AppConfig {
    type Error = anyhow::Error;

    fn try_from(value: MdvArgs) -> Result<Self> {
        let watch = value.watches();
        let MdvArgs { command, path, theme, no_mermaid, scroll, zoom, colors, .. } = value;

        if command.is_some() {
            bail!("view configuration cannot be built from a subcommand")
        }

        let Some(path) = path else { bail!("mdv requires a Markdown file path") };

        let mermaid_mode = if no_mermaid { MermaidMode::Disabled } else { MermaidMode::Enabled };

        Ok(Self {
            path,
            watch,
            theme: theme.resolve(),
            mermaid_mode,
            scroll,
            zoom_percent: zoom,
            colors,
            palette: None,
        })
    }
}

impl AppConfig {
    #[must_use]
    pub fn with_terminal_palette(self, palette: Option<TerminalPalette>) -> Self {
        match (self.colors, palette) {
            (ColorSource::Terminal, Some(palette)) => Self {
                theme: if palette.is_dark() { Theme::Dark } else { Theme::Light },
                palette: Some(palette),
                ..self
            },
            _ => self,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MermaidMode {
    Enabled,
    Disabled,
}
