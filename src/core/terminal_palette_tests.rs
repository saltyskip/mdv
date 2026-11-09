use super::*;

const REX_REPLIES: &[u8] = b"\x1b]10;rgb:b8b8/cdcd/d9d9\x1b\\\x1b]11;rgb:0606/1010/1a1a\x1b\\\x1b]4;4;rgb:4a4a/9090/c9c9\x1b\\\x1b]4;12;rgb:6d6d/abab/dddd\x1b\\\x1b[?62;22c";

#[test]
fn parses_the_colors_rex_reports() {
    let palette = TerminalPalette::from_replies(REX_REPLIES)
        .unwrap_or_else(|| panic!("Rex replies should parse"));

    assert_eq!(palette.foreground, Rgb::new(0xb8, 0xcd, 0xd9));
    assert_eq!(palette.background, Rgb::new(0x06, 0x10, 0x1a));
    assert_eq!(palette.blue, Some(Rgb::new(0x4a, 0x90, 0xc9)));
    assert_eq!(palette.link(), Rgb::new(0x6d, 0xab, 0xdd));
    assert!(palette.is_dark());
    assert!(TerminalPalette::replies_complete(REX_REPLIES));
}

#[test]
fn accepts_bel_terminated_and_short_channel_replies() {
    let replies = b"\x1b]10;rgb:00/00/00\x07\x1b]11;rgb:f/f/f\x07\x1b[?1;2c";
    let palette =
        TerminalPalette::from_replies(replies).unwrap_or_else(|| panic!("replies should parse"));

    assert_eq!(palette.foreground, Rgb::new(0, 0, 0));
    assert_eq!(palette.background, Rgb::new(255, 255, 255));
    assert_eq!(palette.link(), palette.foreground);
    assert!(!palette.is_dark());
}

#[test]
fn a_terminal_that_answers_only_the_device_query_has_no_palette() {
    let replies = b"\x1b[?62;22c";

    assert!(TerminalPalette::replies_complete(replies));
    assert!(TerminalPalette::from_replies(replies).is_none());
    assert!(!TerminalPalette::replies_complete(b"\x1b]10;rgb:00/00/00\x1b\\"));
}

#[test]
fn mixing_moves_each_channel_toward_the_other_color() {
    let black = Rgb::new(0, 0, 0);
    let white = Rgb::new(255, 255, 255);

    assert_eq!(black.mix(white, 0.0), black);
    assert_eq!(black.mix(white, 1.0), white);
    assert_eq!(black.mix(white, 0.5).hex(), "#808080");
}
