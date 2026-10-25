use serde::{Deserialize, Serialize};

use crate::Error;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImagePoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, schemars::JsonSchema)]
pub struct DesktopPoint {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Geometry comes from the native capture, not a tool caller. ScreenCaptureKit's
/// image is display-oriented; the bounds are in the same top-left desktop space.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Geometry {
    image_width: u32,
    image_height: u32,
    bounds: Rect,
    rotation: u16,
}

impl Geometry {
    pub fn new(width: u32, height: u32, bounds: Rect, rotation: u16) -> Result<Self, Error> {
        if width == 0 || height == 0 || width.max(height) > 1920 {
            return Err(Error::Geometry("capture dimensions must be 1..1920 pixels"));
        }
        if ![bounds.x, bounds.y, bounds.width, bounds.height]
            .into_iter()
            .all(f64::is_finite)
            || bounds.width <= 0.0
            || bounds.height <= 0.0
            || !(bounds.x + bounds.width).is_finite()
            || !(bounds.y + bounds.height).is_finite()
        {
            return Err(Error::Geometry(
                "desktop bounds must be finite and nonempty",
            ));
        }
        if !matches!(rotation, 0 | 90 | 180 | 270) {
            return Err(Error::Geometry("display rotation must be a right angle"));
        }
        Ok(Self {
            image_width: width,
            image_height: height,
            bounds,
            rotation,
        })
    }

    pub fn desktop(&self, point: ImagePoint) -> Result<DesktopPoint, Error> {
        if !point.x.is_finite()
            || !point.y.is_finite()
            || point.x < 0.0
            || point.y < 0.0
            || point.x >= f64::from(self.image_width)
            || point.y >= f64::from(self.image_height)
        {
            return Err(Error::Input("point is outside the captured image"));
        }
        Ok(DesktopPoint {
            x: self.bounds.x + point.x / f64::from(self.image_width) * self.bounds.width,
            y: self.bounds.y + point.y / f64::from(self.image_height) * self.bounds.height,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bounds() -> Rect {
        Rect {
            x: -1440.0,
            y: -100.0,
            width: 1440.0,
            height: 900.0,
        }
    }

    #[test]
    fn image_coordinates_map_to_negative_origin_desktop_points() {
        let geometry = Geometry::new(960, 600, bounds(), 0).unwrap();
        assert_eq!(
            geometry.desktop(ImagePoint { x: 480.0, y: 300.0 }).unwrap(),
            DesktopPoint {
                x: -720.0,
                y: 350.0
            }
        );
        assert_eq!(
            geometry.desktop(ImagePoint { x: 0.0, y: 0.0 }).unwrap(),
            DesktopPoint {
                x: -1440.0,
                y: -100.0
            }
        );
    }

    #[test]
    fn display_oriented_retina_and_rotated_captures_use_recorded_bounds() {
        let geometry = Geometry::new(
            1200,
            1920,
            Rect {
                x: 100.0,
                y: 0.0,
                width: 600.0,
                height: 960.0,
            },
            90,
        )
        .unwrap();
        assert_eq!(
            geometry.desktop(ImagePoint { x: 600.0, y: 960.0 }).unwrap(),
            DesktopPoint { x: 400.0, y: 480.0 }
        );
    }

    #[test]
    fn invalid_geometry_and_out_of_frame_points_never_become_desktop_input() {
        for width in [0, 1921, u32::MAX] {
            assert!(Geometry::new(width, 600, bounds(), 0).is_err());
        }
        assert!(Geometry::new(960, 600, bounds(), 45).is_err());
        assert!(
            Geometry::new(
                960,
                600,
                Rect {
                    width: f64::NAN,
                    ..bounds()
                },
                0
            )
            .is_err()
        );
        let geometry = Geometry::new(960, 600, bounds(), 0).unwrap();
        for point in [
            ImagePoint { x: -1.0, y: 0.0 },
            ImagePoint { x: 960.0, y: 0.0 },
            ImagePoint { x: 0.0, y: 600.0 },
            ImagePoint {
                x: f64::NAN,
                y: 0.0,
            },
            ImagePoint {
                x: 0.0,
                y: f64::INFINITY,
            },
        ] {
            assert!(geometry.desktop(point).is_err());
        }
    }
}
