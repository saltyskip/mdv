use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use crate::{
    cli::Theme,
    core::{
        config::MermaidMode,
        terminal_palette::{Rgb, TerminalPalette},
    },
};

use super::{
    build_github_html, build_github_html_with_palette,
    mermaid::{decorate_mermaid_svg, render_mermaid_blocks_with},
};

#[test]
fn decorate_mermaid_svg_adds_renderer_class() {
    let decorated = decorate_mermaid_svg(r#"<svg viewBox="0 0 10 10"><rect /></svg>"#);

    assert!(decorated.starts_with(r#"<svg class="mdv-mermaid-diagram""#));
    assert!(decorated.contains(r#"viewBox="0 0 10 10""#));
}

#[test]
fn render_mermaid_blocks_reuses_duplicate_sources() {
    let calls = Arc::new(AtomicUsize::new(0));
    let call_counter = Arc::clone(&calls);
    let sources =
        vec!["graph TD\n    A --> B\n".to_string(), "graph TD\n    A --> B\n".to_string()];

    let rendered = render_mermaid_blocks_with(&sources, MermaidMode::Enabled, move |content| {
        call_counter.fetch_add(1, Ordering::SeqCst);
        Ok(format!("<svg>{}</svg>", content.trim()))
    })
    .unwrap_or_else(|error| panic!("render should succeed: {error}"));

    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(rendered.len(), 1);
    assert!(rendered.values().next().is_some_and(|html| html.contains("mdv-mermaid")));
}

#[test]
fn front_matter_renders_as_a_key_value_table() {
    let source = "---\nstatus: \"proposed\"\ndate: 2026-10-19\nconsulted: { the approver }\n---\n\n# Title\n";

    let html =
        build_github_html(source, std::path::Path::new("."), Theme::Dark, MermaidMode::Disabled)
            .unwrap_or_else(|error| panic!("html should build: {error}"));

    assert!(html.contains("<tr><th>status</th><td>proposed</td></tr>"));
    assert!(html.contains("<tr><th>date</th><td>2026-10-19</td></tr>"));
    assert!(html.contains("<tr><th>consulted</th><td>{ the approver }</td></tr>"));
    assert!(!html.contains("<hr"));
    assert!(html.find("mdv-front-matter") < html.find("<h1"));
}

#[test]
fn a_terminal_palette_replaces_the_github_colors() {
    let palette = TerminalPalette {
        foreground: Rgb::new(0xb8, 0xcd, 0xd9),
        background: Rgb::new(0x06, 0x10, 0x1a),
        blue: Some(Rgb::new(0x4a, 0x90, 0xc9)),
        bright_blue: Some(Rgb::new(0x6d, 0xab, 0xdd)),
    };

    let html = build_github_html_with_palette(
        "# Title\n\n[link](https://example.com)\n",
        std::path::Path::new("."),
        Theme::Dark,
        MermaidMode::Disabled,
        Some(palette),
    )
    .unwrap_or_else(|error| panic!("html should build: {error}"));

    assert!(html.contains("background: #06101a"));
    assert!(html.contains("#b8cdd9"));
    assert!(html.contains("#6dabdd"));
    assert!(!html.to_lowercase().contains("#0d1117"));
    assert!(!html.to_lowercase().contains("#f0f6fc"));
}
