#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    #[must_use]
    pub const fn new(red: u8, green: u8, blue: u8) -> Self {
        Self { red, green, blue }
    }

    #[must_use]
    pub fn parse_x11(spec: &str) -> Option<Self> {
        let channels = spec.strip_prefix("rgb:")?;
        let mut parts = channels.split('/').map(scale_hex_channel);
        let color = Self::new(parts.next()??, parts.next()??, parts.next()??);
        parts.next().is_none().then_some(color)
    }

    #[must_use]
    pub fn mix(self, other: Self, weight_of_other: f32) -> Self {
        let blend = |from: u8, to: u8| {
            (f32::from(from) + (f32::from(to) - f32::from(from)) * weight_of_other).round() as u8
        };
        Self::new(
            blend(self.red, other.red),
            blend(self.green, other.green),
            blend(self.blue, other.blue),
        )
    }

    #[must_use]
    pub fn relative_luminance(self) -> f32 {
        let linear = |channel: u8| {
            let value = f32::from(channel) / 255.0;
            if value <= 0.040_45 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * linear(self.red) + 0.7152 * linear(self.green) + 0.0722 * linear(self.blue)
    }

    #[must_use]
    pub fn hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}", self.red, self.green, self.blue)
    }
}

fn scale_hex_channel(digits: &str) -> Option<u8> {
    if digits.is_empty() || digits.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    let max = (1_u32 << (4 * digits.len())) - 1;
    Some(((value * 255 + max / 2) / max) as u8)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TerminalPalette {
    pub foreground: Rgb,
    pub background: Rgb,
    pub blue: Option<Rgb>,
    pub bright_blue: Option<Rgb>,
}

impl TerminalPalette {
    pub const QUERY: &'static str =
        "\u{1b}]10;?\u{1b}\\\u{1b}]11;?\u{1b}\\\u{1b}]4;4;?\u{1b}\\\u{1b}]4;12;?\u{1b}\\\u{1b}[c";

    #[must_use]
    pub fn from_replies(replies: &[u8]) -> Option<Self> {
        let text = String::from_utf8_lossy(replies);
        let reply = |prefix: &str| {
            text.split('\u{1b}')
                .filter_map(|sequence| sequence.strip_prefix(']'))
                .find_map(|body| body.strip_prefix(prefix))
                .map(|spec| spec.trim_end_matches(['\\', '\u{7}']))
                .and_then(Rgb::parse_x11)
        };
        Some(Self {
            foreground: reply("10;")?,
            background: reply("11;")?,
            blue: reply("4;4;"),
            bright_blue: reply("4;12;"),
        })
    }

    #[must_use]
    pub fn replies_complete(replies: &[u8]) -> bool {
        let Some(start) = replies.windows(3).rposition(|window| window == b"\x1b[?") else {
            return false;
        };
        replies[start..].contains(&b'c')
    }

    #[must_use]
    pub fn is_dark(self) -> bool {
        self.background.relative_luminance() < 0.18
    }

    #[must_use]
    pub fn link(self) -> Rgb {
        self.bright_blue.or(self.blue).unwrap_or(self.foreground)
    }

    #[must_use]
    pub fn accent(self) -> Rgb {
        self.blue.or(self.bright_blue).unwrap_or(self.foreground)
    }

    #[must_use]
    pub fn muted(self) -> Rgb {
        self.foreground.mix(self.background, 0.35)
    }

    #[must_use]
    pub fn border(self) -> Rgb {
        self.background.mix(self.foreground, 0.22)
    }

    #[must_use]
    pub fn surface(self) -> Rgb {
        self.background.mix(self.foreground, 0.06)
    }

    #[must_use]
    pub fn inline_code(self) -> Rgb {
        self.background.mix(self.foreground, 0.4)
    }
}

#[cfg(test)]
#[path = "terminal_palette_tests.rs"]
mod tests;
