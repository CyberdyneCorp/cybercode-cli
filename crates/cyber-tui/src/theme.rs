//! Built-in themes (`tui` → Themes and appearance).

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub text: Color,
    pub muted: Color,
    pub accent: Color,
    pub user: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    pub border: Color,
    pub selection: Color,
    pub code: Color,
}

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

const fn theme(name: &'static str, c: [u32; 10]) -> Theme {
    Theme {
        name,
        text: rgb(c[0]),
        muted: rgb(c[1]),
        accent: rgb(c[2]),
        user: rgb(c[3]),
        success: rgb(c[4]),
        warning: rgb(c[5]),
        error: rgb(c[6]),
        border: rgb(c[7]),
        selection: rgb(c[8]),
        code: rgb(c[9]),
    }
}

/// `system` uses the terminal's own palette.
const SYSTEM: Theme = Theme {
    name: "system",
    text: Color::Reset,
    muted: Color::DarkGray,
    accent: Color::Cyan,
    user: Color::Blue,
    success: Color::Green,
    warning: Color::Yellow,
    error: Color::Red,
    border: Color::DarkGray,
    selection: Color::DarkGray,
    code: Color::Magenta,
};

pub const THEMES: [Theme; 12] = [
    theme(
        "cyber",
        [
            0xd6e4f0, 0x6b7a8f, 0x00e5ff, 0x7aa2f7, 0x7ee787, 0xffcc66, 0xff6b6b, 0x2b3445,
            0x1f2a3a, 0xc792ea,
        ],
    ),
    SYSTEM,
    theme(
        "dracula",
        [
            0xf8f8f2, 0x6272a4, 0xbd93f9, 0x8be9fd, 0x50fa7b, 0xf1fa8c, 0xff5555, 0x44475a,
            0x44475a, 0xff79c6,
        ],
    ),
    theme(
        "tokyonight",
        [
            0xc0caf5, 0x565f89, 0x7aa2f7, 0x7dcfff, 0x9ece6a, 0xe0af68, 0xf7768e, 0x292e42,
            0x283457, 0xbb9af7,
        ],
    ),
    theme(
        "catppuccin",
        [
            0xcdd6f4, 0x6c7086, 0xcba6f7, 0x89b4fa, 0xa6e3a1, 0xf9e2af, 0xf38ba8, 0x313244,
            0x45475a, 0xf5c2e7,
        ],
    ),
    theme(
        "gruvbox",
        [
            0xebdbb2, 0x928374, 0xfe8019, 0x83a598, 0xb8bb26, 0xfabd2f, 0xfb4934, 0x3c3836,
            0x504945, 0xd3869b,
        ],
    ),
    theme(
        "nord",
        [
            0xeceff4, 0x4c566a, 0x88c0d0, 0x81a1c1, 0xa3be8c, 0xebcb8b, 0xbf616a, 0x3b4252,
            0x434c5e, 0xb48ead,
        ],
    ),
    theme(
        "one-dark",
        [
            0xabb2bf, 0x5c6370, 0x61afef, 0x56b6c2, 0x98c379, 0xe5c07b, 0xe06c75, 0x3e4451,
            0x3e4451, 0xc678dd,
        ],
    ),
    theme(
        "github-light",
        [
            0x24292f, 0x6e7781, 0x0969da, 0x8250df, 0x1a7f37, 0x9a6700, 0xcf222e, 0xd0d7de,
            0xddf4ff, 0x953800,
        ],
    ),
    theme(
        "solarized-light",
        [
            0x586e75, 0x93a1a1, 0x268bd2, 0x2aa198, 0x859900, 0xb58900, 0xdc322f, 0xeee8d5,
            0xeee8d5, 0xd33682,
        ],
    ),
    theme(
        "monokai",
        [
            0xf8f8f2, 0x75715e, 0x66d9ef, 0xae81ff, 0xa6e22e, 0xe6db74, 0xf92672, 0x3e3d32,
            0x49483e, 0xfd971f,
        ],
    ),
    theme(
        "everforest",
        [
            0xd3c6aa, 0x859289, 0x7fbbb3, 0x83c092, 0xa7c080, 0xdbbc7f, 0xe67e80, 0x374145,
            0x414b50, 0xd699b6,
        ],
    ),
];

pub fn by_name(name: &str) -> Theme {
    THEMES
        .iter()
        .copied()
        .find(|t| t.name == name)
        .unwrap_or(THEMES[0])
}
