use bitflags::bitflags;

/// The driver takes five bytes: buttons, dx, dy, wheel and one reserved byte.
pub const MOUSE_REPORT_LEN: usize = 5;

bitflags! {
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct MouseButtons: u8 {
        const LEFT    = 0x01;
        const RIGHT   = 0x02;
        const MIDDLE  = 0x04;
        const BACK    = 0x08;
        const FORWARD = 0x10;
    }
}

/// Only the buttons are ever filled in: pointer movement and the wheel already
/// work without us, and repeating them would only add lag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MouseReport {
    buttons: MouseButtons,
    dx: i8,
    dy: i8,
    wheel: i8,
}

impl MouseReport {
    pub const EMPTY: Self = Self {
        buttons: MouseButtons::empty(),
        dx: 0,
        dy: 0,
        wheel: 0,
    };

    #[must_use]
    pub const fn buttons(buttons: MouseButtons) -> Self {
        Self {
            buttons,
            dx: 0,
            dy: 0,
            wheel: 0,
        }
    }

    #[must_use]
    pub fn to_bytes(self) -> [u8; MOUSE_REPORT_LEN] {
        [
            self.buttons.bits(),
            self.dx as u8,
            self.dy as u8,
            self.wheel as u8,
            0,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_report_releases_every_button() {
        assert_eq!(MouseReport::EMPTY.to_bytes(), [0; MOUSE_REPORT_LEN]);
    }

    #[test]
    fn buttons_are_a_bitmap_in_the_first_byte() {
        let report = MouseReport::buttons(MouseButtons::LEFT | MouseButtons::MIDDLE);

        assert_eq!(report.to_bytes(), [0x05, 0, 0, 0, 0]);
    }

    #[test]
    fn several_buttons_held_at_once_are_reported_together() {
        let all = MouseButtons::all();
        let report = MouseReport::buttons(all);

        assert_eq!(report.to_bytes()[0], all.bits());
    }
}
