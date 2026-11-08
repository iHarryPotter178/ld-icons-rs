use serde::{Deserialize, Serialize};

pub const ICON_SIZE: i32 = 64;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub x: i32,
    pub y: i32,
}

pub fn snap_axis(value: f64, origin: i32, spacing: i32) -> i32 {
    let spacing = spacing.max(1);
    let relative = (value - f64::from(origin)) / f64::from(spacing);
    (f64::from(origin) + relative.round() * f64::from(spacing))
        .max(0.0)
        .min(f64::from(i32::MAX)) as i32
}

pub fn hit_icon(icons: &[Position], x: f64, y: f64) -> Option<usize> {
    hit_icon_except(icons, x, y, usize::MAX)
}

pub fn hit_icon_except(icons: &[Position], x: f64, y: f64, excluded: usize) -> Option<usize> {
    icons.iter().enumerate().find_map(|(index, position)| {
        (index != excluded
            && x >= f64::from(position.x)
            && x < f64::from(position.x + ICON_SIZE)
            && y >= f64::from(position.y)
            && y < f64::from(position.y + ICON_SIZE))
        .then_some(index)
    })
}

pub fn clamp_icon(position: Position, width: i32, height: i32) -> Position {
    let max_x = (width - ICON_SIZE).max(0);
    let max_y = (height - ICON_SIZE).max(0);
    Position {
        x: position.x.max(0).min(max_x),
        y: position.y.max(0).min(max_y),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_positions_snap_to_grid() {
        assert_eq!(snap_axis(118.0, 50, 140), 50);
        assert_eq!(snap_axis(131.0, 50, 140), 190);
    }

    #[test]
    fn hit_testing_only_covers_icon_rectangles() {
        let icons = [Position { x: 50, y: 50 }, Position { x: 190, y: 50 }];
        assert_eq!(hit_icon(&icons, 60.0, 60.0), Some(0));
        assert_eq!(hit_icon(&icons, 120.0, 60.0), None);
        assert_eq!(hit_icon_except(&icons, 200.0, 60.0, 0), Some(1));
    }
}
