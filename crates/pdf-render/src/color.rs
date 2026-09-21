use pdf_paint::{Color, ColorSpace, Colorant, IccAlternate, IndexedBase};

use crate::Approximation;
use crate::cmyk_samples::CMYK_SAMPLES;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Resolved {
    taken: [Option<Approximation>; 2],
}

impl Resolved {
    pub const EXACT: Self = Self {
        taken: [None, None],
    };

    #[must_use]
    pub const fn approximated(kind: Approximation) -> Self {
        Self {
            taken: [Some(kind), None],
        }
    }

    #[must_use]
    pub fn and(self, other: Self) -> Self {
        let mut merged = self;
        for kind in other.approximations() {
            if merged.taken.contains(&Some(kind)) {
                continue;
            }
            if let Some(free) = merged.taken.iter_mut().find(|held| held.is_none()) {
                *free = Some(kind);
            }
        }
        merged
    }

    #[must_use]
    pub fn is_exact(self) -> bool {
        self.taken.iter().all(Option::is_none)
    }

    pub fn approximations(self) -> impl Iterator<Item = Approximation> {
        self.taken.into_iter().flatten()
    }
}

#[must_use]
pub fn is_invisible(space: &ColorSpace) -> bool {
    match space {
        ColorSpace::Separation(definition) => definition.colorant.value == Colorant::None,
        ColorSpace::DeviceN(definition) => {
            let colorants = &definition.colorants.value;
            !colorants.is_empty() && colorants.iter().all(|name| *name == Colorant::None)
        }
        _ => false,
    }
}

#[must_use]
pub fn color_to_rgb(space: &ColorSpace, color: &Color) -> Option<([f64; 3], Resolved)> {
    let components = match color {
        Color::DeviceGray(gray) | Color::CalGray(gray) => vec![*gray],
        Color::DeviceRgb(red, green, blue) | Color::CalRgb(red, green, blue) => {
            vec![*red, *green, *blue]
        }
        Color::DeviceCmyk(cyan, magenta, yellow, black) => {
            vec![*cyan, *magenta, *yellow, *black]
        }
        Color::Lab(lightness, a, b) => vec![*lightness, *a, *b],
        Color::IccBased(components) | Color::DeviceN(components) => components.clone(),
        Color::Indexed(index) => vec![f64::from(*index)],
        Color::Separation(tint) => vec![*tint],
        Color::PatternUnspecified | Color::TilingPattern(_) | Color::ShadingPattern(_) => {
            return None;
        }
    };
    components_to_rgb(space, &components)
}

#[must_use]
pub fn components_to_rgb(space: &ColorSpace, components: &[f64]) -> Option<([f64; 3], Resolved)> {
    match space {
        ColorSpace::DeviceGray => {
            let gray = *components.first()?;
            Some(([gray, gray, gray], Resolved::EXACT))
        }
        ColorSpace::CalGray(_) => {
            let gray = *components.first()?;
            Some((
                [gray, gray, gray],
                Resolved::approximated(Approximation::CalibratedAsDevice),
            ))
        }
        ColorSpace::DeviceRgb => Some((triple(components)?, Resolved::EXACT)),
        ColorSpace::CalRgb(_) => Some((
            triple(components)?,
            Resolved::approximated(Approximation::CalibratedAsDevice),
        )),
        ColorSpace::DeviceCmyk => {
            let [cyan, magenta, yellow, black] = quad(components)?;
            Some((
                cmyk_to_rgb(cyan, magenta, yellow, black),
                Resolved::approximated(Approximation::UnmanagedCmyk),
            ))
        }
        ColorSpace::Lab(definition) => {
            let [lightness, star_a, star_b] = triple(components)?;
            Some((
                lab_to_srgb(lightness, star_a, star_b, definition.white_point.value),
                Resolved::EXACT,
            ))
        }
        ColorSpace::IccBased(definition) => {
            if let Some(transform) = crate::icc::transform_for(&definition.profile)
                && let Some(rgb) = transform.to_srgb(components)
            {
                return Some((rgb, Resolved::EXACT));
            }
            let (rgb, alternate) = alternate_to_rgb(&definition.alternate.value, components)?;
            Some((
                rgb,
                Resolved::approximated(Approximation::IccAlternate).and(alternate),
            ))
        }
        ColorSpace::Indexed(definition) => {
            let index = *components.first()?;
            let index = palette_index(index, definition.hival.value);
            let base = definition.base_components(index);
            indexed_base_to_rgb(&definition.base.value, &base)
        }
        ColorSpace::Separation(definition) => {
            let tint = *components.first()?;
            let alternate = definition.alternate_components(tint)?;
            let (rgb, resolved) = components_to_rgb(&definition.alternate.value, &alternate)?;
            if definition.colorant.value == Colorant::All {
                return Some((
                    rgb,
                    Resolved::approximated(Approximation::TintTransform).and(resolved),
                ));
            }
            Some((rgb, resolved))
        }
        ColorSpace::DeviceN(definition) => {
            let alternate = definition.alternate_components(components)?;
            components_to_rgb(&definition.alternate.value, &alternate)
        }
        ColorSpace::Pattern(_) => None,
    }
}

fn palette_index(value: f64, hival: u8) -> u8 {
    if value.is_nan() || value < 0.5 {
        return 0;
    }
    (0..hival)
        .find(|candidate| value < f64::from(*candidate) + 0.5)
        .unwrap_or(hival)
}

fn indexed_base_to_rgb(base: &IndexedBase, components: &[f64]) -> Option<([f64; 3], Resolved)> {
    match base {
        IndexedBase::DeviceGray => components_to_rgb(&ColorSpace::DeviceGray, components),
        IndexedBase::DeviceRgb => components_to_rgb(&ColorSpace::DeviceRgb, components),
        IndexedBase::DeviceCmyk => components_to_rgb(&ColorSpace::DeviceCmyk, components),
        IndexedBase::CalGray(space) => {
            components_to_rgb(&ColorSpace::CalGray(space.clone()), components)
        }
        IndexedBase::CalRgb(space) => {
            components_to_rgb(&ColorSpace::CalRgb(space.clone()), components)
        }
        IndexedBase::Lab(space) => components_to_rgb(&ColorSpace::Lab(space.clone()), components),
        IndexedBase::IccBased(space) => {
            components_to_rgb(&ColorSpace::IccBased(space.clone()), components)
        }
        IndexedBase::Separation(space) => {
            components_to_rgb(&ColorSpace::Separation(space.clone()), components)
        }
        IndexedBase::DeviceN(space) => {
            components_to_rgb(&ColorSpace::DeviceN(space.clone()), components)
        }
    }
}

fn alternate_to_rgb(alternate: &IccAlternate, components: &[f64]) -> Option<([f64; 3], Resolved)> {
    match alternate {
        IccAlternate::DeviceGray => components_to_rgb(&ColorSpace::DeviceGray, components),
        IccAlternate::DeviceRgb => components_to_rgb(&ColorSpace::DeviceRgb, components),
        IccAlternate::DeviceCmyk => components_to_rgb(&ColorSpace::DeviceCmyk, components),
        IccAlternate::CalGray(space) => {
            components_to_rgb(&ColorSpace::CalGray(space.clone()), components)
        }
        IccAlternate::CalRgb(space) => {
            components_to_rgb(&ColorSpace::CalRgb(space.clone()), components)
        }
        IccAlternate::Lab(space) => components_to_rgb(&ColorSpace::Lab(space.clone()), components),
    }
}

fn triple(components: &[f64]) -> Option<[f64; 3]> {
    match components {
        [first, second, third] => Some([*first, *second, *third]),
        _ => None,
    }
}

fn quad(components: &[f64]) -> Option<[f64; 4]> {
    match components {
        [first, second, third, fourth] => Some([*first, *second, *third, *fourth]),
        _ => None,
    }
}

fn cmyk_sample(position: i32, channel: usize) -> i32 {
    #[allow(clippy::cast_sign_loss)]
    let index = position.max(0) as usize * 3 + channel;
    i32::from(CMYK_SAMPLES[index])
}

const CMYK_STRIDE: [i32; 4] = [9 * 9 * 9, 9 * 9, 9, 1];

fn cmyk_to_rgb(cyan: f64, magenta: f64, yellow: f64, black: f64) -> [f64; 3] {
    fn level(component: f64) -> i32 {
        #[allow(clippy::cast_possible_truncation)]
        let narrowed = component.clamp(0.0, 1.0) as f32;
        #[allow(clippy::cast_possible_truncation)]
        let rounded = (narrowed * 255.0 + 0.499_999_97) as i32;
        rounded.clamp(0, 255)
    }

    let fixed =
        [level(cyan), level(magenta), level(yellow), level(black)].map(|component| component << 8);
    let near = fixed.map(|value| (value + 4096) >> 13);
    let far = [0, 1, 2, 3].map(|axis| {
        let truncated = fixed[axis] >> 13;
        if truncated != near[axis] {
            truncated
        } else if near[axis] == 8 {
            near[axis] - 1
        } else {
            near[axis] + 1
        }
    });
    let position = near
        .iter()
        .zip(CMYK_STRIDE)
        .map(|(index, stride)| index * stride)
        .sum::<i32>();

    let mut result = [0.0_f64; 3];
    for (channel, out) in result.iter_mut().enumerate() {
        let mut total = cmyk_sample(position, channel) << 8;
        for axis in 0..4 {
            let neighbour = position + (far[axis] - near[axis]) * CMYK_STRIDE[axis];
            let rate = (fixed[axis] - (near[axis] << 13)) * (near[axis] - far[axis]);
            total += (cmyk_sample(position, channel) - cmyk_sample(neighbour, channel)) * rate / 32;
        }
        *out = f64::from(total.max(0) >> 8) / 255.0;
    }
    result
}

fn lab_to_srgb(lightness: f64, star_a: f64, star_b: f64, white: [f64; 3]) -> [f64; 3] {
    let middle = (lightness + 16.0) / 116.0;
    let upper = middle + star_a / 500.0;
    let lower = middle - star_b / 200.0;
    xyz_to_srgb([
        white[0] * lab_inverse(upper),
        white[1] * lab_inverse(middle),
        white[2] * lab_inverse(lower),
    ])
}

pub(crate) fn xyz_d50_to_srgb(xyz: [f64; 3]) -> [f64; 3] {
    const BRADFORD_D50_TO_D65: [[f64; 3]; 3] = [
        [0.955_476_6, -0.023_039_3, 0.063_163_6],
        [-0.028_289_5, 1.009_941_6, 0.021_007_7],
        [0.012_298_2, -0.020_483_0, 1.329_909_8],
    ];
    xyz_to_srgb(apply(BRADFORD_D50_TO_D65, xyz))
}

fn xyz_to_srgb(xyz: [f64; 3]) -> [f64; 3] {
    const XYZ_D65_TO_LINEAR_SRGB: [[f64; 3]; 3] = [
        [3.240_97, -1.537_383, -0.498_611],
        [-0.969_244, 1.875_968, 0.041_555],
        [0.055_630, -0.203_977, 1.056_972],
    ];
    apply(XYZ_D65_TO_LINEAR_SRGB, xyz).map(srgb_transfer)
}

fn apply(matrix: [[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    matrix.map(|row| {
        row.iter()
            .zip(vector)
            .fold(0.0, |sum, (coefficient, value)| {
                coefficient.mul_add(value, sum)
            })
    })
}

pub(crate) const PCS_D50: [f64; 3] = [0.964_2, 1.0, 0.824_9];

pub(crate) fn pcs_lab_to_xyz(lightness: f64, star_a: f64, star_b: f64) -> [f64; 3] {
    let middle = (lightness + 16.0) / 116.0;
    let upper = middle + star_a / 500.0;
    let lower = middle - star_b / 200.0;
    [
        PCS_D50[0] * lab_inverse(upper),
        PCS_D50[1] * lab_inverse(middle),
        PCS_D50[2] * lab_inverse(lower),
    ]
}

pub(crate) fn pcs_neutral_of_lightness(lightness: f64) -> [f64; 3] {
    let scale = lab_inverse((lightness + 16.0) / 116.0);
    PCS_D50.map(|axis| axis * scale)
}

pub(crate) fn pcs_lightness(luminance: f64) -> f64 {
    116.0f64.mul_add(lab_forward(luminance), -16.0)
}

fn lab_forward(value: f64) -> f64 {
    const THRESHOLD: f64 = 6.0 / 29.0;
    if value > THRESHOLD * THRESHOLD * THRESHOLD {
        value.cbrt()
    } else {
        value / (3.0 * THRESHOLD * THRESHOLD) + 4.0 / 29.0
    }
}

fn lab_inverse(value: f64) -> f64 {
    const THRESHOLD: f64 = 6.0 / 29.0;
    if value > THRESHOLD {
        value * value * value
    } else {
        3.0 * THRESHOLD * THRESHOLD * (value - 4.0 / 29.0)
    }
}

fn srgb_transfer(linear: f64) -> f64 {
    let linear = linear.clamp(0.0, 1.0);
    if linear <= 0.003_130_8 {
        12.92 * linear
    } else {
        1.055_f64.mul_add(linear.powf(1.0 / 2.4), -0.055)
    }
}

#[cfg(test)]
mod tests {
    use super::Resolved;
    use crate::Approximation;

    #[test]
    fn a_resolution_keeps_both_shortcuts_and_repeats_neither() {
        let icc = Resolved::approximated(Approximation::IccAlternate);
        let cmyk = Resolved::approximated(Approximation::UnmanagedCmyk);
        assert!(Resolved::EXACT.is_exact());
        assert!(!icc.is_exact());
        let both = icc.and(cmyk);
        assert_eq!(
            both.approximations().collect::<Vec<_>>(),
            vec![Approximation::IccAlternate, Approximation::UnmanagedCmyk]
        );
        assert_eq!(
            icc.and(icc).approximations().collect::<Vec<_>>(),
            vec![Approximation::IccAlternate],
            "a shortcut named twice is one shortcut"
        );
        assert_eq!(
            both.and(Resolved::approximated(Approximation::StrokeJoins))
                .approximations()
                .count(),
            2,
            "a third has nowhere to go, and this is where that is visible"
        );
    }

    use super::cmyk_to_rgb;

    fn levels(cyan: f64, magenta: f64, yellow: f64, black: f64) -> [u8; 3] {
        cmyk_to_rgb(cyan, magenta, yellow, black).map(|channel| {
            #[allow(clippy::cast_possible_truncation)]
            let narrowed = channel.clamp(0.0, 1.0) as f32;
            crate::to_byte(narrowed)
        })
    }

    #[test]
    fn the_ink_corners_are_the_measured_ones_and_not_the_spec_formula() {
        assert_eq!(levels(0.0, 0.0, 0.0, 0.0), [255, 255, 255]);
        assert_eq!(levels(1.0, 0.0, 0.0, 0.0), [0, 174, 239]);
        assert_eq!(levels(0.0, 1.0, 0.0, 0.0), [237, 2, 140]);
        assert_eq!(levels(0.0, 0.0, 1.0, 0.0), [255, 241, 1]);
        assert_eq!(levels(0.0, 0.0, 0.0, 1.0), [35, 31, 32]);
        assert_eq!(levels(1.0, 1.0, 1.0, 0.0), [54, 53, 57]);
        assert_eq!(levels(1.0, 1.0, 1.0, 1.0), [0, 0, 0]);
    }

    #[test]
    fn the_middle_of_the_cube_is_sampled_rather_than_assumed() {
        assert_eq!(levels(0.0, 0.0, 0.5, 0.0), [255, 247, 154]);
        assert_eq!(levels(0.5, 0.5, 0.5, 0.5), [82, 74, 71]);
        assert_eq!(levels(0.1, 0.2, 0.3, 0.4), [150, 135, 118]);
    }

    #[test]
    fn the_port_reproduces_pdfium_across_the_whole_cube() {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        let mut combinations = 0_u32;
        for cyan in (0..=255_u32).step_by(17) {
            for magenta in (0..=255_u32).step_by(17) {
                for yellow in (0..=255_u32).step_by(17) {
                    for black in (0..=255_u32).step_by(17) {
                        let axes =
                            [cyan, magenta, yellow, black].map(|level| f64::from(level) / 255.0);
                        for channel in levels(axes[0], axes[1], axes[2], axes[3]) {
                            hash = (hash ^ u64::from(channel)).wrapping_mul(0x0100_0000_01b3);
                        }
                        combinations += 1;
                    }
                }
            }
        }
        assert_eq!(combinations, 65_536);
        assert_eq!(hash, 0x27b7_2b4f_2192_012e);
    }

    #[test]
    fn components_outside_the_range_are_clamped_rather_than_extrapolated() {
        assert_eq!(levels(-1.0, 0.0, 0.0, 0.0), levels(0.0, 0.0, 0.0, 0.0));
        assert_eq!(levels(2.0, 0.0, 0.0, 0.0), levels(1.0, 0.0, 0.0, 0.0));
    }
}
