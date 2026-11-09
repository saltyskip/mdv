use std::path::Path;

use anyhow::{Context, Result};
use comrak::{Arena, format_html_with_plugins, options::Plugins, parse_document};
use tracing::info_span;

use crate::{
    cli::Theme,
    core::{config::MermaidMode, terminal_palette::TerminalPalette},
    io::mermaid_cli::MermaidCliRenderer,
    render::markdown_pipeline::gfm_options,
};

mod mermaid;
mod postprocess;
mod styles;
#[cfg(test)]
mod tests;

use self::{
    mermaid::replace_mermaid_code_blocks,
    postprocess::{
        decorate_code_blocks, inject_alert_icons, restore_supported_raw_html, retint_code_tokens,
    },
    styles::{directory_url, syntax_adapter, theme_styles},
};

pub fn build_github_html(
    source: &str,
    base_dir: &Path,
    theme: Theme,
    mermaid_mode: MermaidMode,
) -> Result<String> {
    build_github_html_with_palette(source, base_dir, theme, mermaid_mode, None)
}

pub fn build_github_html_with_palette(
    source: &str,
    base_dir: &Path,
    theme: Theme,
    mermaid_mode: MermaidMode,
    palette: Option<TerminalPalette>,
) -> Result<String> {
    let _span = info_span!("github_html.build").entered();
    let mut options = gfm_options();
    options.extension.front_matter_delimiter = Some("---".to_owned());
    options.render.escape = true;
    options.render.r#unsafe = false;
    options.render.tasklist_classes = true;
    options.render.github_pre_lang = true;

    let arena = Arena::new();
    let root = parse_document(&arena, source, &options);
    replace_mermaid_code_blocks(root, mermaid_mode, theme, &MermaidCliRenderer::from_env())?;

    let mut plugins = Plugins::default();
    plugins.render.codefence_syntax_highlighter = Some(syntax_adapter(theme));

    let mut rendered = String::new();
    format_html_with_plugins(root, &options, &mut rendered, &plugins)
        .context("formatting markdown to html failed")?;

    let body_html = restore_supported_raw_html(&decorate_code_blocks(&retint_code_tokens(
        &inject_alert_icons(&rendered),
        theme,
    )));
    let body_html = format!("{}{body_html}", front_matter_table(root));
    let styles = theme_styles(theme, palette);
    let base_href = escape_attribute(&directory_url(base_dir)?);

    Ok(format!(
        r#"<!DOCTYPE html>
<html lang="en" data-color-mode="{color_mode}" data-dark-theme="dark" data-light-theme="light">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <base href="{base_href}" />
  <style>{styles}</style>
</head>
<body>
  <div class="mdv-page">
    <article class="markdown-body entry-content container-lg">
      {body_html}
    </article>
  </div>
</body>
</html>"#,
        color_mode = theme.as_str(),
        base_href = base_href,
        styles = styles,
    ))
}

fn front_matter_table<'a>(root: &'a comrak::nodes::AstNode<'a>) -> String {
    let Some(front_matter) = root.children().find_map(|node| match &node.data.borrow().value {
        comrak::nodes::NodeValue::FrontMatter(text) => Some(text.clone()),
        _ => None,
    }) else {
        return String::new();
    };
    let rows: String = front_matter_fields(&front_matter)
        .into_iter()
        .map(|(key, value)| {
            format!(
                "<tr><th>{}</th><td>{}</td></tr>",
                escape_attribute(&key),
                escape_attribute(&value)
            )
        })
        .collect();
    if rows.is_empty() {
        return String::new();
    }
    format!("<table class=\"mdv-front-matter\"><tbody>{rows}</tbody></table>\n")
}

fn front_matter_fields(front_matter: &str) -> Vec<(String, String)> {
    front_matter.lines().filter(|line| line.trim() != "---" && !line.trim().is_empty()).fold(
        Vec::<(String, String)>::new(),
        |mut fields, line| {
            let top_level = !line.starts_with([' ', '\t']);
            match (top_level, line.split_once(':'), fields.last_mut()) {
                (true, Some((key, value)), _) => {
                    fields.push((key.trim().to_owned(), unquote(value.trim()).to_owned()));
                }
                (_, _, Some((_, value))) => {
                    if !value.is_empty() {
                        value.push(' ');
                    }
                    value.push_str(line.trim());
                }
                _ => {}
            }
            fields
        },
    )
}

fn unquote(value: &str) -> &str {
    ['"', '\'']
        .iter()
        .find_map(|quote| value.strip_prefix(*quote).and_then(|inner| inner.strip_suffix(*quote)))
        .unwrap_or(value)
}

fn escape_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
