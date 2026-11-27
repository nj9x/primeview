use egui::Color32;

#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color32,
    pub grid: Color32,
    pub separator: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub crosshair: Color32,
    pub label_bg: Color32,
    pub label_text: Color32,
    pub up: Color32,
    pub down: Color32,
    pub accent: Color32,
    pub drawing: Color32,
    pub dark: bool,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            bg: Color32::from_rgb(14, 16, 21),
            grid: Color32::from_rgb(28, 31, 39),
            separator: Color32::from_rgb(44, 48, 58),
            text: Color32::from_rgb(214, 218, 226),
            text_dim: Color32::from_rgb(132, 138, 150),
            crosshair: Color32::from_rgb(118, 124, 138),
            label_bg: Color32::from_rgb(52, 57, 70),
            label_text: Color32::WHITE,
            up: Color32::from_rgb(46, 189, 133),
            down: Color32::from_rgb(229, 72, 77),
            accent: Color32::from_rgb(76, 141, 255),
            drawing: Color32::from_rgb(76, 141, 255),
            dark: true,
        }
    }

    pub fn light() -> Self {
        Self {
            bg: Color32::from_rgb(255, 255, 255),
            grid: Color32::from_rgb(240, 242, 245),
            separator: Color32::from_rgb(214, 218, 224),
            text: Color32::from_rgb(28, 32, 40),
            text_dim: Color32::from_rgb(110, 116, 128),
            crosshair: Color32::from_rgb(140, 146, 158),
            label_bg: Color32::from_rgb(40, 44, 54),
            label_text: Color32::WHITE,
            up: Color32::from_rgb(22, 160, 110),
            down: Color32::from_rgb(214, 52, 58),
            accent: Color32::from_rgb(30, 100, 235),
            drawing: Color32::from_rgb(30, 100, 235),
            dark: false,
        }
    }

    pub fn for_dark_mode(dark: bool) -> Self {
        if dark { Self::dark() } else { Self::light() }
    }

    pub fn candle(&self, up: bool) -> Color32 {
        if up { self.up } else { self.down }
    }
}

pub fn rgba(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

pub fn with_alpha(c: Color32, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), a)
}
