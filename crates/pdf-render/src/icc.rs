pub(crate) fn transform_for(profile: &std::sync::Arc<[u8]>) -> Option<IccTransform> {
    const KEPT: usize = 8;
    type Remembered = Vec<(std::sync::Arc<[u8]>, Option<IccTransform>)>;
    thread_local! {
        static CACHE: std::cell::RefCell<Remembered> =
            const { std::cell::RefCell::new(Vec::new()) };
    }
    CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((_, held)) = cache
            .iter()
            .find(|(bytes, _)| std::sync::Arc::ptr_eq(bytes, profile))
        {
            return held.clone();
        }
        let built = IccTransform::of(profile);
        if cache.len() >= KEPT {
            cache.remove(0);
        }
        cache.push((std::sync::Arc::clone(profile), built.clone()));
        built
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct IccTransform {
    shape: Shape,
    black: [f64; 3],
}

#[derive(Clone, Debug, PartialEq)]
enum Shape {
    MatrixTrc(Box<MatrixTrc>),
    Lut(Box<Lut>),
    GrayTrc(Box<Curve>),
}

#[derive(Clone, Debug, PartialEq)]
struct MatrixTrc {
    curves: [Curve; 3],
    colourants: [[f64; 3]; 3],
}

#[derive(Clone, Debug, PartialEq)]
enum Curve {
    Identity,
    Gamma(f64),
    Sampled(Vec<f64>),
    Parametric(Parametric),
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Parametric {
    kind: u16,
    values: [f64; 7],
}

impl Curve {
    fn light(&self, value: f64) -> f64 {
        let value = value.clamp(0.0, 1.0);
        match self {
            Self::Identity => value,
            Self::Gamma(gamma) => value.powf(*gamma),
            Self::Sampled(samples) => interpolate(samples, value),
            Self::Parametric(curve) => curve.light(value),
        }
    }
}

impl Parametric {
    fn light(self, input: f64) -> f64 {
        let [
            gamma,
            slope,
            offset,
            linear_slope,
            break_point,
            shift,
            linear_shift,
        ] = self.values;
        let power = |base: f64| if base > 0.0 { base.powf(gamma) } else { 0.0 };
        let shaped = slope.mul_add(input, offset);
        match self.kind {
            0 => power(input),
            1 => {
                if shaped >= 0.0 {
                    power(shaped)
                } else {
                    0.0
                }
            }
            2 => {
                if shaped >= 0.0 {
                    power(shaped) + shift
                } else {
                    shift
                }
            }
            3 => {
                if input >= break_point {
                    power(shaped)
                } else {
                    linear_slope * input
                }
            }
            4 => {
                if input >= break_point {
                    power(shaped) + shift
                } else {
                    linear_slope.mul_add(input, linear_shift)
                }
            }
            _ => input,
        }
    }
}

fn interpolate(samples: &[f64], at: f64) -> f64 {
    let last = samples.len() - 1;
    #[allow(clippy::cast_precision_loss)]
    let position = at * last as f64;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let lower = (position.floor() as usize).min(last);
    let upper = (lower + 1).min(last);
    let fraction = position - position.floor();
    samples[upper].mul_add(fraction, samples[lower] * (1.0 - fraction))
}

impl IccTransform {
    #[must_use]
    pub fn of(profile: &[u8]) -> Option<Self> {
        let tags = tag_table(profile)?;
        let find = |signature: &[u8; 4]| {
            tags.iter()
                .find(|(tag, _, _)| tag == signature)
                .and_then(|(_, offset, size)| {
                    profile.get(*offset as usize..(*offset as usize).checked_add(*size as usize)?)
                })
        };
        let curves = [b"rTRC", b"gTRC", b"bTRC"].map(|signature| find(signature).and_then(curve));
        let colourants =
            [b"rXYZ", b"gXYZ", b"bXYZ"].map(|signature| find(signature).and_then(colourant));
        let [red_curve, green_curve, blue_curve] = curves;
        let [red, green, blue] = colourants;
        let data_space: [u8; 4] = profile.get(16..20)?.try_into().ok()?;
        if let (
            Some(red_curve),
            Some(green_curve),
            Some(blue_curve),
            Some(red),
            Some(green),
            Some(blue),
        ) = (red_curve, green_curve, blue_curve, red, green, blue)
        {
            return Some(Self::assembled(
                Shape::MatrixTrc(Box::new(MatrixTrc {
                    curves: [red_curve, green_curve, blue_curve],
                    colourants: [red, green, blue],
                })),
                data_space,
            ));
        }

        let pcs_is_lab = match profile.get(20..24)? {
            b"Lab " => true,
            b"XYZ " => false,
            _ => return None,
        };
        for signature in [b"A2B0", b"A2B1", b"A2B2"] {
            if let Some(lut) = find(signature).and_then(|tag| Lut::of(tag, pcs_is_lab)) {
                return Some(Self::assembled(Shape::Lut(Box::new(lut)), data_space));
            }
        }

        if !pcs_is_lab && let Some(curve) = find(b"kTRC").and_then(curve) {
            return Some(Self::assembled(Shape::GrayTrc(Box::new(curve)), data_space));
        }
        None
    }

    fn assembled(shape: Shape, data_space: [u8; 4]) -> Self {
        let darkest: &[f64] = match &data_space {
            b"GRAY" => &[0.0],
            b"RGB " => &[0.0, 0.0, 0.0],
            b"CMYK" => &[1.0, 1.0, 1.0, 1.0],
            _ => &[],
        };
        let mut built = Self {
            shape,
            black: [0.0; 3],
        };
        if let Some(black) = built.to_pcs(darkest) {
            let lightness = crate::color::pcs_lightness(black[1]).clamp(0.0, 50.0);
            built.black = crate::color::pcs_neutral_of_lightness(lightness);
        }
        built
    }

    #[must_use]
    pub fn to_srgb(&self, components: &[f64]) -> Option<[f64; 3]> {
        let xyz = self.to_pcs(components)?;
        Some(crate::color::xyz_d50_to_srgb(self.compensated(xyz)))
    }

    fn to_pcs(&self, components: &[f64]) -> Option<[f64; 3]> {
        match &self.shape {
            Shape::MatrixTrc(matrix) => {
                let (curves, colourants) = (&matrix.curves, &matrix.colourants);
                let [red, green, blue] = match components {
                    [red, green, blue] => [*red, *green, *blue],
                    _ => return None,
                };
                let light = [
                    curves[0].light(red),
                    curves[1].light(green),
                    curves[2].light(blue),
                ];
                let mut xyz = [0.0_f64; 3];
                for (channel, value) in light.iter().enumerate() {
                    for (axis, slot) in xyz.iter_mut().enumerate() {
                        *slot = colourants[channel][axis].mul_add(*value, *slot);
                    }
                }
                Some(xyz)
            }
            Shape::Lut(lut) => lut.to_pcs(components),
            Shape::GrayTrc(curve) => {
                let [gray] = match components {
                    [gray] => [*gray],
                    _ => return None,
                };
                let luminance = curve.light(gray);
                Some(crate::color::PCS_D50.map(|axis| axis * luminance))
            }
        }
    }

    fn compensated(&self, xyz: [f64; 3]) -> [f64; 3] {
        let mut out = xyz;
        for (axis, slot) in out.iter_mut().enumerate() {
            let white = crate::color::PCS_D50[axis];
            let black = self.black[axis];
            if black <= 0.0 || black >= white {
                continue;
            }
            let scale = white / (white - black);
            *slot = scale.mul_add(*slot, white * (1.0 - scale));
        }
        out
    }
}

fn tag_table(profile: &[u8]) -> Option<Vec<([u8; 4], u32, u32)>> {
    let count = u32::from_be_bytes(profile.get(128..132)?.try_into().ok()?) as usize;
    if count > profile.len().saturating_sub(132) / 12 {
        return None;
    }
    (0..count)
        .map(|index| {
            let at = 132 + index * 12;
            let entry = profile.get(at..at + 12)?;
            Some((
                entry[0..4].try_into().ok()?,
                u32::from_be_bytes(entry[4..8].try_into().ok()?),
                u32::from_be_bytes(entry[8..12].try_into().ok()?),
            ))
        })
        .collect()
}

fn fixed(bytes: &[u8]) -> Option<f64> {
    let raw = i32::from_be_bytes(bytes.get(0..4)?.try_into().ok()?);
    Some(f64::from(raw) / 65536.0)
}

fn colourant(tag: &[u8]) -> Option<[f64; 3]> {
    if tag.get(0..4)? != b"XYZ " {
        return None;
    }
    Some([
        fixed(tag.get(8..)?)?,
        fixed(tag.get(12..)?)?,
        fixed(tag.get(16..)?)?,
    ])
}

fn curve(tag: &[u8]) -> Option<Curve> {
    match tag.get(0..4)? {
        b"curv" => {
            let count = u32::from_be_bytes(tag.get(8..12)?.try_into().ok()?) as usize;
            match count {
                0 => Some(Curve::Identity),
                1 => {
                    let raw = u16::from_be_bytes(tag.get(12..14)?.try_into().ok()?);
                    Some(Curve::Gamma(f64::from(raw) / 256.0))
                }
                _ => {
                    let samples = tag.get(12..12 + count.checked_mul(2)?)?;
                    Some(Curve::Sampled(
                        samples
                            .chunks_exact(2)
                            .map(|pair| f64::from(u16::from_be_bytes([pair[0], pair[1]])) / 65535.0)
                            .collect(),
                    ))
                }
            }
        }
        b"para" => {
            let kind = u16::from_be_bytes(tag.get(8..10)?.try_into().ok()?);
            let wanted = match kind {
                0 => 1,
                1 => 3,
                2 => 4,
                3 => 5,
                4 => 7,
                _ => return None,
            };
            let mut values = [0.0_f64; 7];
            for (index, slot) in values.iter_mut().enumerate().take(wanted) {
                *slot = fixed(tag.get(12 + index * 4..)?)?;
            }
            Some(Curve::Parametric(Parametric { kind, values }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Curve, IccTransform, Parametric};

    pub(super) fn fixed(value: f64) -> [u8; 4] {
        #[allow(clippy::cast_possible_truncation)]
        let raw = (value * 65536.0).round() as i32;
        raw.to_be_bytes()
    }

    fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
        let mut tag = b"XYZ \0\0\0\0".to_vec();
        tag.extend(fixed(x));
        tag.extend(fixed(y));
        tag.extend(fixed(z));
        tag
    }

    fn srgb_curve() -> Vec<u8> {
        let mut tag = b"para\0\0\0\0".to_vec();
        tag.extend(3_u16.to_be_bytes());
        tag.extend(0_u16.to_be_bytes());
        for value in [2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.040_45] {
            tag.extend(fixed(value));
        }
        tag
    }

    fn profile(tags: &[(&'static [u8; 4], Vec<u8>)]) -> Vec<u8> {
        profile_with_pcs(*b"XYZ ", tags)
    }

    pub(super) fn profile_with_pcs(pcs: [u8; 4], tags: &[(&'static [u8; 4], Vec<u8>)]) -> Vec<u8> {
        profile_with_spaces([0; 4], pcs, tags)
    }

    pub(super) fn profile_with_spaces(
        data_space: [u8; 4],
        pcs: [u8; 4],
        tags: &[(&'static [u8; 4], Vec<u8>)],
    ) -> Vec<u8> {
        let mut header = vec![0_u8; 128];
        header[16..20].copy_from_slice(&data_space);
        header[20..24].copy_from_slice(&pcs);
        header.extend(u32::try_from(tags.len()).unwrap().to_be_bytes());
        let mut offset = u32::try_from(132 + tags.len() * 12).unwrap();
        let mut table = Vec::new();
        let mut data = Vec::new();
        for (signature, bytes) in tags {
            table.extend_from_slice(*signature);
            table.extend(offset.to_be_bytes());
            table.extend(u32::try_from(bytes.len()).unwrap().to_be_bytes());
            offset += u32::try_from(bytes.len()).unwrap();
            data.extend_from_slice(bytes);
        }
        header.extend(table);
        header.extend(data);
        header
    }

    fn srgb_profile() -> Vec<u8> {
        profile(&srgb_tags())
    }

    pub(super) fn srgb_tags() -> Vec<(&'static [u8; 4], Vec<u8>)> {
        vec![
            (b"rXYZ", xyz_tag(0.436_07, 0.222_49, 0.013_92)),
            (b"gXYZ", xyz_tag(0.385_15, 0.716_87, 0.097_10)),
            (b"bXYZ", xyz_tag(0.143_06, 0.060_61, 0.714_10)),
            (b"rTRC", srgb_curve()),
            (b"gTRC", srgb_curve()),
            (b"bTRC", srgb_curve()),
        ]
    }

    fn levels(transform: &IccTransform, components: [f64; 3]) -> [u8; 3] {
        transform.to_srgb(&components).unwrap().map(|channel| {
            #[allow(clippy::cast_possible_truncation)]
            let narrowed = channel.clamp(0.0, 1.0) as f32;
            crate::to_byte(narrowed)
        })
    }

    #[test]
    fn an_srgb_profile_is_the_identity_through_the_whole_conversion() {
        let transform = IccTransform::of(&srgb_profile()).expect("a matrix/TRC profile");
        for channel in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let grey = levels(&transform, [channel, channel, channel]);
            #[allow(clippy::cast_possible_truncation)]
            let expected = crate::to_byte(channel as f32);
            assert!(
                grey.iter().all(|level| level.abs_diff(expected) <= 1),
                "{channel} gave {grey:?}, wanted {expected}"
            );
        }
        assert_eq!(levels(&transform, [1.0, 0.0, 0.0]), [255, 0, 0]);
        assert_eq!(levels(&transform, [0.0, 1.0, 0.0]), [0, 255, 0]);
        assert_eq!(levels(&transform, [0.0, 0.0, 1.0]), [0, 0, 255]);
        assert_eq!(levels(&transform, [1.0, 1.0, 1.0]), [255, 255, 255]);
        assert_eq!(levels(&transform, [0.0, 0.0, 0.0]), [0, 0, 0]);
    }

    #[test]
    fn a_profile_that_is_not_srgb_moves_colour_exactly_where_its_own_parts_say() {
        let linear = {
            let mut tag = b"curv\0\0\0\0".to_vec();
            tag.extend(0_u32.to_be_bytes());
            tag
        };
        let scene = profile(&[
            (b"rXYZ", xyz_tag(0.436_07, 0.222_49, 0.013_92)),
            (b"gXYZ", xyz_tag(0.385_15, 0.716_87, 0.097_10)),
            (b"bXYZ", xyz_tag(0.143_06, 0.060_61, 0.714_10)),
            (b"rTRC", linear.clone()),
            (b"gTRC", linear.clone()),
            (b"bTRC", linear),
        ]);
        let transform = IccTransform::of(&scene).expect("a matrix/TRC profile");
        let grey = levels(&transform, [0.5, 0.5, 0.5]);
        assert!(
            grey.iter().all(|level| level.abs_diff(188) <= 1),
            "half the light is 188 in sRGB, not {grey:?}"
        );
        assert_eq!(levels(&transform, [1.0, 1.0, 1.0]), [255, 255, 255]);
        assert_eq!(levels(&transform, [0.0, 0.0, 0.0]), [0, 0, 0]);

        let swapped = profile(&[
            (b"rXYZ", xyz_tag(0.143_06, 0.060_61, 0.714_10)),
            (b"gXYZ", xyz_tag(0.385_15, 0.716_87, 0.097_10)),
            (b"bXYZ", xyz_tag(0.436_07, 0.222_49, 0.013_92)),
            (b"rTRC", srgb_curve()),
            (b"gTRC", srgb_curve()),
            (b"bTRC", srgb_curve()),
        ]);
        let transform = IccTransform::of(&swapped).expect("a matrix/TRC profile");
        assert_eq!(levels(&transform, [1.0, 0.0, 0.0]), [0, 0, 255]);
        assert_eq!(levels(&transform, [0.0, 0.0, 1.0]), [255, 0, 0]);
        assert_eq!(levels(&transform, [0.0, 1.0, 0.0]), [0, 255, 0]);
        assert_eq!(levels(&transform, [1.0, 1.0, 1.0]), [255, 255, 255]);
    }

    #[test]
    fn a_profile_that_is_not_matrix_trc_is_refused_rather_than_approximated() {
        assert!(IccTransform::of(&profile(&[(b"A2B0", vec![0; 32])])).is_none());
        assert!(
            IccTransform::of(&profile_with_pcs(*b"Lab ", &[(b"kTRC", srgb_curve())])).is_none()
        );
        assert!(
            IccTransform::of(&profile(&[
                (b"rXYZ", xyz_tag(0.4, 0.2, 0.0)),
                (b"gXYZ", xyz_tag(0.4, 0.7, 0.1)),
                (b"rTRC", srgb_curve()),
                (b"gTRC", srgb_curve()),
                (b"bTRC", srgb_curve()),
            ]))
            .is_none()
        );
        let mut truncated = srgb_profile();
        truncated.truncate(200);
        assert!(IccTransform::of(&truncated).is_none());
    }

    #[test]
    fn a_monochrome_profile_is_its_one_curve_on_the_neutral_axis() {
        let transform =
            IccTransform::of(&profile(&[(b"kTRC", srgb_curve())])).expect("a monochrome profile");
        for (input, want) in [(0.0, 0), (0.5, 128), (1.0, 255)] {
            let rgb = transform.to_srgb(&[input]).expect("one component");
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let levels = rgb.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            assert!(
                levels.iter().all(|level| level.abs_diff(want) <= 1),
                "{input} through sRGB's own curve is {want}, not {levels:?}"
            );
        }
        assert!(transform.to_srgb(&[0.5, 0.5, 0.5]).is_none());

        let linear = {
            let mut tag = b"curv\0\0\0\0".to_vec();
            tag.extend(0_u32.to_be_bytes());
            tag
        };
        let transform =
            IccTransform::of(&profile(&[(b"kTRC", linear)])).expect("a monochrome profile");
        let rgb = transform.to_srgb(&[0.5]).expect("one component");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let levels = rgb.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
        assert!(
            levels.iter().all(|level| level.abs_diff(188) <= 1),
            "half the light is 188 in sRGB, not {levels:?}"
        );
    }

    #[test]
    fn a_profile_whose_black_is_not_black_is_compensated_the_way_pdfium_is() {
        let lifted = {
            let mut tag = b"curv\0\0\0\0".to_vec();
            tag.extend(256_u32.to_be_bytes());
            for index in 0..256_u32 {
                let black = 1570.0;
                #[allow(
                    clippy::cast_precision_loss,
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss
                )]
                let value = (black + (65535.0 - black) * f64::from(index) / 255.0) as u16;
                tag.extend(value.to_be_bytes());
            }
            tag
        };

        let declared = profile_with_spaces(*b"GRAY", *b"XYZ ", &[(b"kTRC", lifted.clone())]);
        let transform = IccTransform::of(&declared).expect("a monochrome profile");
        let level = |value: f64| {
            let rgb = transform.to_srgb(&[value]).expect("one component");
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let levels = rgb.map(|channel| (channel.clamp(0.0, 1.0) * 255.0).round() as u8);
            assert!(
                levels.iter().all(|channel| *channel == levels[0]),
                "a monochrome profile answers on the neutral axis, not {levels:?}"
            );
            levels[0]
        };
        assert_eq!(level(0.0), 0, "the profile's own black is the page's black");
        assert_eq!(level(1.0), 255, "and its white is untouched");

        let undeclared = profile_with_pcs(*b"XYZ ", &[(b"kTRC", lifted)]);
        let plain = IccTransform::of(&undeclared).expect("a monochrome profile");
        let rgb = plain.to_srgb(&[0.0]).expect("one component");
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let raised = (rgb[0].clamp(0.0, 1.0) * 255.0).round() as u8;
        assert!(
            raised.abs_diff(43) <= 1,
            "an uncompensated black of 0.024 is level 43, not {raised}"
        );
    }

    #[test]
    fn a_tag_count_larger_than_the_profile_is_refused_before_it_is_indexed() {
        let mut lying = srgb_profile();
        lying[128..132].copy_from_slice(&1_000_000_u32.to_be_bytes());
        assert!(IccTransform::of(&lying).is_none());
        assert!(IccTransform::of(&[0_u8; 64]).is_none());
        assert!(IccTransform::of(&[]).is_none());
    }

    #[test]
    fn a_sampled_curve_interpolates_between_its_samples_and_at_both_ends() {
        let curve = Curve::Sampled(vec![0.0, 0.25, 1.0]);
        assert!((curve.light(0.0) - 0.0).abs() < 1e-12);
        assert!((curve.light(0.5) - 0.25).abs() < 1e-12);
        assert!((curve.light(1.0) - 1.0).abs() < 1e-12);
        assert!((curve.light(0.25) - 0.125).abs() < 1e-12);
        assert!((curve.light(2.0) - 1.0).abs() < 1e-12);
        assert!((curve.light(-1.0) - 0.0).abs() < 1e-12);
        assert!(curve.light(f64::NAN).is_nan() || curve.light(f64::NAN) == 0.0);
    }

    #[test]
    fn a_negative_base_never_reaches_powf_and_becomes_a_nan() {
        for kind in [1, 2] {
            let curve = Curve::Parametric(Parametric {
                kind,
                values: [2.4, -4.0, 0.1, 0.0, 0.0, 0.0, 0.0],
            });
            assert!(curve.light(0.9).is_finite(), "type {kind} produced a NaN");
        }
    }

    #[test]
    fn the_component_count_must_be_the_one_the_profile_takes() {
        let transform = IccTransform::of(&srgb_profile()).unwrap();
        assert!(transform.to_srgb(&[0.5]).is_none());
        assert!(transform.to_srgb(&[0.5, 0.5, 0.5, 0.5]).is_none());
        assert!(transform.to_srgb(&[]).is_none());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Lut {
    inputs: usize,
    grid: usize,
    input_tables: Vec<Vec<f64>>,
    clut: Vec<f64>,
    output_tables: Vec<Vec<f64>>,
    pcs: Pcs,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Pcs {
    Xyz,
    LegacyLab16,
    LegacyLab8,
}

impl Pcs {
    fn to_xyz(self, values: [f64; 3]) -> [f64; 3] {
        match self {
            Self::Xyz => values.map(|value| value * 65535.0 / 32768.0),
            Self::LegacyLab16 => {
                let scale = 65535.0 / 65280.0;
                crate::color::pcs_lab_to_xyz(
                    values[0] * scale * 100.0,
                    values[1].mul_add(scale * 255.0, -128.0),
                    values[2].mul_add(scale * 255.0, -128.0),
                )
            }
            Self::LegacyLab8 => crate::color::pcs_lab_to_xyz(
                values[0] * 100.0,
                values[1].mul_add(255.0, -128.0),
                values[2].mul_add(255.0, -128.0),
            ),
        }
    }
}

impl Lut {
    fn of(tag: &[u8], pcs_is_lab: bool) -> Option<Self> {
        const IDENTITY: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

        let kind = tag.get(0..4)?;
        let sixteen = match kind {
            b"mft1" => false,
            b"mft2" => true,
            _ => return None,
        };
        let inputs = usize::from(*tag.get(8)?);
        let outputs = usize::from(*tag.get(9)?);
        let grid = usize::from(*tag.get(10)?);
        if outputs != 3 || !(1..=4).contains(&inputs) || !(2..=255).contains(&grid) {
            return None;
        }

        let mut matrix = [0.0_f64; 9];
        for (index, slot) in matrix.iter_mut().enumerate() {
            *slot = fixed(tag.get(12 + index * 4..16 + index * 4)?)?;
        }
        if matrix
            .iter()
            .zip(IDENTITY)
            .any(|(value, expected)| (value - expected).abs() > 1e-9)
        {
            return None;
        }

        let (input_entries, output_entries, header) = if sixteen {
            let read = |at: usize| -> Option<usize> {
                Some(usize::from(u16::from_be_bytes(
                    tag.get(at..at + 2)?.try_into().ok()?,
                )))
            };
            (read(48)?, read(50)?, 52)
        } else {
            (256, 256, 48)
        };
        if input_entries < 2 || output_entries < 2 {
            return None;
        }

        let width = if sixteen { 2 } else { 1 };
        let scale = if sixteen { 65535.0 } else { 255.0 };
        let value_at = |at: usize| -> Option<f64> {
            let raw = if sixteen {
                f64::from(u16::from_be_bytes(tag.get(at..at + 2)?.try_into().ok()?))
            } else {
                f64::from(*tag.get(at)?)
            };
            Some(raw / scale)
        };

        let mut at = header;
        let mut input_tables = Vec::with_capacity(inputs);
        for _ in 0..inputs {
            let mut table = Vec::with_capacity(input_entries);
            for index in 0..input_entries {
                table.push(value_at(at + index * width)?);
            }
            at += input_entries * width;
            input_tables.push(table);
        }

        let points = grid.checked_pow(u32::try_from(inputs).ok()?)?;
        let cells = points.checked_mul(3)?;
        if cells > 4 * 1024 * 1024 {
            return None;
        }
        let mut clut = Vec::with_capacity(cells);
        for index in 0..cells {
            clut.push(value_at(at + index * width)?);
        }
        at += cells * width;

        let mut output_tables = Vec::with_capacity(3);
        for _ in 0..3 {
            let mut table = Vec::with_capacity(output_entries);
            for index in 0..output_entries {
                table.push(value_at(at + index * width)?);
            }
            at += output_entries * width;
            output_tables.push(table);
        }

        let pcs = match (pcs_is_lab, sixteen) {
            (false, _) => Pcs::Xyz,
            (true, true) => Pcs::LegacyLab16,
            (true, false) => Pcs::LegacyLab8,
        };
        Some(Self {
            inputs,
            grid,
            input_tables,
            clut,
            output_tables,
            pcs,
        })
    }

    fn to_pcs(&self, components: &[f64]) -> Option<[f64; 3]> {
        if components.len() != self.inputs {
            return None;
        }
        let mut lower = [0_usize; 4];
        let mut fraction = [0.0_f64; 4];
        for (axis, component) in components.iter().enumerate() {
            let shaped = interpolate(&self.input_tables[axis], component.clamp(0.0, 1.0));
            #[allow(clippy::cast_precision_loss)]
            let position = shaped.clamp(0.0, 1.0) * (self.grid - 1) as f64;
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let floor = (position.floor() as usize).min(self.grid - 1);
            lower[axis] = floor.min(self.grid.saturating_sub(2));
            #[allow(clippy::cast_precision_loss)]
            let floor_value = lower[axis] as f64;
            fraction[axis] = position - floor_value;
        }

        let mut blended = [0.0_f64; 3];
        for corner in 0..(1_usize << self.inputs) {
            let mut weight = 1.0;
            let mut offset = 0_usize;
            for axis in 0..self.inputs {
                let upper = corner & (1 << axis) != 0;
                weight *= if upper {
                    fraction[axis]
                } else {
                    1.0 - fraction[axis]
                };
                let index = lower[axis] + usize::from(upper);
                offset = offset * self.grid + index.min(self.grid - 1);
            }
            if weight == 0.0 {
                continue;
            }
            for (channel, slot) in blended.iter_mut().enumerate() {
                *slot += weight * self.clut.get(offset * 3 + channel)?;
            }
        }

        let mut out = [0.0_f64; 3];
        for (channel, slot) in out.iter_mut().enumerate() {
            *slot = interpolate(
                &self.output_tables[channel],
                blended[channel].clamp(0.0, 1.0),
            );
        }
        Some(self.pcs.to_xyz(out))
    }
}

#[cfg(test)]
mod lut_tests {
    use super::IccTransform;
    use super::tests::profile_with_pcs;

    fn lightness_ramp_tag(sixteen_bit: bool) -> Vec<u8> {
        let mut tag = if sixteen_bit {
            b"mft2\0\0\0\0".to_vec()
        } else {
            b"mft1\0\0\0\0".to_vec()
        };
        tag.extend([4, 3, 2, 0]);
        for value in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
            tag.extend(super::tests::fixed(value));
        }
        let entries: usize = if sixteen_bit { 2 } else { 256 };
        if sixteen_bit {
            tag.extend(2_u16.to_be_bytes());
            tag.extend(2_u16.to_be_bytes());
        }
        let push = |tag: &mut Vec<u8>, value: u16| {
            if sixteen_bit {
                tag.extend(value.to_be_bytes());
            } else {
                tag.push(u8::try_from(value >> 8).unwrap());
            }
        };
        for _ in 0..4 {
            for index in 0..entries {
                let numerator = u32::try_from(index).unwrap() * 65535;
                let value = numerator / u32::try_from(entries - 1).unwrap();
                push(&mut tag, u16::try_from(value).unwrap());
            }
        }
        for cell in 0..16 {
            let cyan_high = cell & 0b1000 != 0;
            push(&mut tag, if cyan_high { 0x0000 } else { 0xFF00 });
            push(&mut tag, 0x8000);
            push(&mut tag, 0x8000);
        }
        for _ in 0..3 {
            for index in 0..entries {
                let numerator = u32::try_from(index).unwrap() * 65535;
                let value = numerator / u32::try_from(entries - 1).unwrap();
                push(&mut tag, u16::try_from(value).unwrap());
            }
        }
        tag
    }

    fn levels(transform: &IccTransform, cmyk: [f64; 4]) -> [u8; 3] {
        let components: Vec<f64> = cmyk.iter().map(|value| value / 255.0).collect();
        transform
            .to_srgb(&components)
            .expect("four components")
            .map(|channel| {
                #[allow(clippy::cast_possible_truncation)]
                let narrowed = channel.clamp(0.0, 1.0) as f32;
                crate::to_byte(narrowed)
            })
    }

    #[test]
    fn a_lookup_table_profile_is_walked_and_its_legacy_lab_decoded() {
        let profile = profile_with_pcs(*b"Lab ", &[(b"A2B0", lightness_ramp_tag(true))]);
        let transform = IccTransform::of(&profile).expect("an mft2 A2B0");
        assert_eq!(levels(&transform, [0.0, 0.0, 0.0, 0.0]), [255, 255, 255]);
        assert_eq!(levels(&transform, [255.0, 0.0, 0.0, 0.0]), [0, 0, 0]);
        assert_eq!(levels(&transform, [127.5, 0.0, 0.0, 0.0]), [119, 119, 119]);
        assert_eq!(
            levels(&transform, [0.0, 255.0, 255.0, 255.0]),
            [255, 255, 255]
        );
        assert_eq!(levels(&transform, [255.0, 255.0, 0.0, 128.0]), [0, 0, 0]);
    }

    #[test]
    fn an_eight_bit_lookup_table_reads_its_own_encoding() {
        let profile = profile_with_pcs(*b"Lab ", &[(b"A2B0", lightness_ramp_tag(false))]);
        let transform = IccTransform::of(&profile).expect("an mft1 A2B0");
        assert_eq!(levels(&transform, [0.0, 0.0, 0.0, 0.0]), [255, 255, 255]);
        assert_eq!(levels(&transform, [255.0, 0.0, 0.0, 0.0]), [0, 0, 0]);
        assert_eq!(levels(&transform, [127.5, 0.0, 0.0, 0.0]), [119, 119, 119]);
    }

    #[test]
    fn a_lookup_table_this_cannot_walk_is_refused_by_name() {
        let mut v4 = lightness_ramp_tag(true);
        v4[0..4].copy_from_slice(b"mAB ");
        assert!(IccTransform::of(&profile_with_pcs(*b"Lab ", &[(b"A2B0", v4)])).is_none());

        let mut tilted = lightness_ramp_tag(true);
        tilted[12..16].copy_from_slice(&super::tests::fixed(2.0));
        assert!(IccTransform::of(&profile_with_pcs(*b"Lab ", &[(b"A2B0", tilted)])).is_none());

        let full = lightness_ramp_tag(true);
        for cut in [1, 20, 60, full.len() - 1] {
            assert!(
                IccTransform::of(&profile_with_pcs(
                    *b"Lab ",
                    &[(b"A2B0", full[..cut].to_vec())]
                ))
                .is_none(),
                "a tag cut to {cut} bytes was accepted"
            );
        }

        assert!(
            IccTransform::of(&profile_with_pcs(
                *b"CMYK",
                &[(b"A2B0", lightness_ramp_tag(true))]
            ))
            .is_none()
        );
    }

    #[test]
    fn a_matrix_profile_is_preferred_to_a_table_in_the_same_profile() {
        let mut tags = super::tests::srgb_tags();
        tags.push((b"A2B0", lightness_ramp_tag(true)));
        let profile = profile_with_pcs(*b"XYZ ", &tags);
        let transform = IccTransform::of(&profile).expect("a profile with both");
        assert!(transform.to_srgb(&[0.5, 0.5, 0.5]).is_some());
    }
}
