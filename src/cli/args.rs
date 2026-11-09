use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Debug, Eq, Parser, PartialEq)]
#[command(
    name = "mdv",
    about = "High-fidelity Markdown viewer for Ghostty and Kitty",
    arg_required_else_help = true,
    args_conflicts_with_subcommands = true
)]
pub struct MdvArgs {
    #[command(subcommand)]
    pub command: Option<MdvCommand>,

    /// Markdown file to open.
    pub path: Option<PathBuf>,

    /// Reload the document when the file changes.
    #[arg(long)]
    pub watch: bool,

    /// Color theme used by the viewer.
    #[arg(long, value_enum, default_value_t = Theme::System)]
    pub theme: Theme,

    /// Disable Mermaid rendering and show placeholders instead.
    #[arg(long = "no-mermaid")]
    pub no_mermaid: bool,

    /// Text size in percent; 100 sets one line of body text to one terminal row.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(50..=300))]
    pub zoom: u16,

    /// How the page moves between scroll positions.
    #[arg(long, value_enum, default_value_t = ScrollStyle::Smooth)]
    pub scroll: ScrollStyle,

    /// Print the current version.
    #[arg(short = 'V', long = "version", global = true)]
    pub version: bool,
}

impl MdvArgs {
    #[must_use]
    pub fn parse_from<I, T>(itr: I) -> Self
    where
        I: IntoIterator<Item = T>,
        T: Into<std::ffi::OsString> + Clone,
    {
        <Self as Parser>::parse_from(itr)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
pub enum MdvCommand {
    /// Replace the current mdv executable when `main`'s CI-generated binary changes.
    #[command(
        visible_alias = "upgrade",
        about = "Replace the current mdv executable when GitHub main publishes a different CI-generated bin/mdv"
    )]
    Update,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum ScrollStyle {
    #[default]
    Smooth,
    Instant,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, ValueEnum)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Theme {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    #[must_use]
    pub fn resolve(self) -> Self {
        match self {
            Self::System => match dark_light::detect() {
                dark_light::Mode::Dark => Self::Dark,
                _ => Self::Light,
            },
            theme => theme,
        }
    }
}
