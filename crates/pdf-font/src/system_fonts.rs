use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::glyph::GlyphProgram;
use crate::sha256;
use crate::substitute::{
    FaceIdentity, FontProvider, FontRequest, FontStyle, SubstitutedFace, SubstitutionReason,
    byte_family_aliases, coverage_faces, family_aliases, generic_faces, normalize_name,
    standard_face_aliases,
};

const ROOTS: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/usr/share/X11/fonts",
    "C:\\Windows\\Fonts",
    "/System/Library/Fonts",
    "/Library/Fonts",
];

const HOME_ROOTS: &[&str] = &[
    ".fonts",
    ".local/share/fonts",
    "Library/Fonts",
    "AppData/Local/Microsoft/Windows/Fonts",
];

const MAX_DEPTH: usize = 6;

const MAX_FILES: usize = 8_192;

const MAX_CACHED_FACES: usize = 12;
const MAX_CACHED_BYTES: usize = 96 * 1024 * 1024;
const MAX_FONT_BYTES: usize = 32 * 1024 * 1024;
const MAX_DECISIONS: usize = 4_096;

type Decision = Option<(PathBuf, u32, String, SubstitutionReason)>;

#[derive(Clone, Debug)]
pub struct PackagedFace {
    pub path: PathBuf,
    pub face_index: u32,
    pub sha256: String,
}

#[derive(Clone, Debug)]
pub struct InstalledFace {
    pub path: PathBuf,
    pub index: u32,
    pub family: String,
    pub subfamily: String,
    pub style: FontStyle,
    pub bytes: u64,
    pub expected_sha256: Option<String>,
}

type FaceKey = (PathBuf, u32);

type LoadedFace = (Arc<GlyphProgram>, Arc<FaceIdentity>, usize);

#[derive(Debug, Default)]
struct Cache {
    loaded: HashMap<FaceKey, LoadedFace>,
    order: Vec<FaceKey>,
    bytes: usize,
    refused: std::collections::HashSet<FaceKey>,
    primary: HashMap<String, Decision>,
    coverage: HashMap<(String, char), Decision>,
}

#[derive(Debug)]
pub struct SystemFontProvider {
    faces: Vec<InstalledFace>,
    by_family: HashMap<String, Vec<usize>>,
    roots: Vec<PathBuf>,
    skipped: usize,
    cache: Mutex<Cache>,
    changed_faces: AtomicUsize,
}

impl SystemFontProvider {
    #[must_use]
    pub fn discover() -> Self {
        let mut roots: Vec<PathBuf> = ROOTS.iter().map(PathBuf::from).collect();
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.extend(HOME_ROOTS.iter().map(|tail| home.join(tail)));
        }
        Self::discover_in(&roots)
    }

    #[must_use]
    pub fn discover_in(roots: &[PathBuf]) -> Self {
        let mut files = Vec::new();
        for root in roots {
            walk(root, 0, &mut files);
            if files.len() >= MAX_FILES {
                break;
            }
        }
        files.sort();
        files.dedup();
        let mut faces = Vec::new();
        let mut skipped = 0_usize;
        for path in files {
            let found = read_faces(&path);
            if found.is_empty() {
                skipped += 1;
                continue;
            }
            faces.extend(found);
        }
        faces.sort_by(|left, right| (&left.path, left.index).cmp(&(&right.path, right.index)));
        let mut by_family: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, face) in faces.iter().enumerate() {
            by_family
                .entry(normalize_name(&face.family))
                .or_default()
                .push(index);
        }
        Self {
            faces,
            by_family,
            roots: roots.to_vec(),
            skipped,
            cache: Mutex::new(Cache::default()),
            changed_faces: AtomicUsize::new(0),
        }
    }

    #[must_use]
    pub fn from_package(faces: &[PackagedFace]) -> (Self, Vec<String>) {
        let mut installed = Vec::new();
        let mut problems = Vec::new();
        for wanted in faces {
            let Some(bytes) = read_program(&wanted.path) else {
                problems.push(format!("{}: unreadable", wanted.path.display()));
                continue;
            };
            let found = sha256::hex(&bytes);
            if found != wanted.sha256 {
                problems.push(format!(
                    "{}: manifest {}, found {found}",
                    wanted.path.display(),
                    wanted.sha256
                ));
                continue;
            }
            let mut found_faces = read_faces_from(
                &mut std::io::Cursor::new(&bytes),
                &wanted.path,
                bytes.len() as u64,
            );
            let Some(position) = found_faces
                .iter()
                .position(|face| face.index == wanted.face_index)
            else {
                problems.push(format!(
                    "{}: no face at index {}",
                    wanted.path.display(),
                    wanted.face_index
                ));
                continue;
            };
            let mut face = found_faces.swap_remove(position);
            face.expected_sha256 = Some(wanted.sha256.clone());
            installed.push(face);
        }
        let mut by_family: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, face) in installed.iter().enumerate() {
            by_family
                .entry(normalize_name(&face.family))
                .or_default()
                .push(index);
        }
        let provider = Self {
            faces: installed,
            by_family,
            roots: Vec::new(),
            skipped: problems.len(),
            cache: Mutex::new(Cache::default()),
            changed_faces: AtomicUsize::new(0),
        };
        (provider, problems)
    }

    #[must_use]
    pub fn faces(&self) -> &[InstalledFace] {
        &self.faces
    }

    #[must_use]
    pub const fn skipped_files(&self) -> usize {
        self.skipped
    }

    #[must_use]
    pub fn changed_faces_refused(&self) -> usize {
        self.changed_faces.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn index_manifest(&self) -> String {
        let roots: Vec<String> = self
            .roots
            .iter()
            .filter(|root| root.is_dir())
            .map(|root| root.display().to_string())
            .collect();
        let changed = self.changed_faces_refused();
        let where_from = if self.roots.is_empty() {
            "packaged".to_owned()
        } else {
            format!("roots [{}]", roots.join(", "))
        };
        format!(
            "{where_from}; {} faces; {} files unreadable{}",
            self.faces.len(),
            self.skipped,
            if changed == 0 {
                String::new()
            } else {
                format!("; {changed} refused after their file changed")
            }
        )
    }

    fn best_in_family(&self, normalized: &str, wanted: FontStyle) -> Option<&InstalledFace> {
        let indices = self.by_family.get(normalized)?;
        indices
            .iter()
            .filter_map(|index| self.faces.get(*index))
            .enumerate()
            .min_by_key(|(order, face)| (face.style.distance(wanted), *order))
            .map(|(_, face)| face)
    }

    fn load(&self, face: &InstalledFace) -> Option<(Arc<GlyphProgram>, Arc<FaceIdentity>)> {
        let key = (face.path.clone(), face.index);
        let mut cache = self.cache.lock().ok()?;
        if cache.refused.contains(&key) {
            return None;
        }
        if let Some((program, identity, _)) = cache.loaded.get(&key) {
            return Some((Arc::clone(program), Arc::clone(identity)));
        }
        let bytes = read_program(&face.path).or_else(|| {
            cache.refused.insert(key.clone());
            None
        })?;
        let size = bytes.len();
        let sha = sha256::hex(&bytes);
        if face
            .expected_sha256
            .as_ref()
            .is_some_and(|expected| expected != &sha)
        {
            self.changed_faces.fetch_add(1, Ordering::Relaxed);
            cache.refused.insert(key);
            return None;
        }
        let Ok(program) = GlyphProgram::parse_face(bytes, face.index) else {
            cache.refused.insert(key.clone());
            return None;
        };
        let program = Arc::new(program);
        let identity = Arc::new(FaceIdentity {
            family: face.family.clone(),
            subfamily: face.subfamily.clone(),
            origin: face.path.display().to_string(),
            sha256: sha,
            face_index: face.index,
            style: face.style,
        });
        cache.loaded.insert(
            key.clone(),
            (Arc::clone(&program), Arc::clone(&identity), size),
        );
        cache.order.push(key);
        cache.bytes += size;
        while cache.order.len() > MAX_CACHED_FACES || cache.bytes > MAX_CACHED_BYTES {
            let oldest = cache.order.remove(0);
            if let Some((_, _, size)) = cache.loaded.remove(&oldest) {
                cache.bytes = cache.bytes.saturating_sub(size);
            }
        }
        Some((program, identity))
    }

    fn first_available(
        &self,
        families: &[&str],
        wanted: FontStyle,
        reason: SubstitutionReason,
    ) -> Option<SubstitutedFace> {
        self.first_matching(families, wanted, reason, |_| true)
    }

    fn first_matching(
        &self,
        families: &[&str],
        wanted: FontStyle,
        reason: SubstitutionReason,
        covers: impl Fn(&GlyphProgram) -> bool,
    ) -> Option<SubstitutedFace> {
        for name in families {
            let normalized = normalize_name(name);
            let Some(face) = self.best_in_family(&normalized, wanted) else {
                continue;
            };
            if let Some((program, identity)) = self.load(face)
                && covers(&program)
            {
                return Some(SubstitutedFace {
                    program,
                    identity,
                    reason,
                });
            }
        }
        None
    }

    fn recall(&self, decision: Decision) -> Option<SubstitutedFace> {
        let (path, index, sha256, reason) = decision?;
        let face = self
            .faces
            .iter()
            .find(|face| face.path == path && face.index == index)?;
        let (program, identity) = self.load(face)?;
        if identity.sha256 != sha256 {
            self.changed_faces.fetch_add(1, Ordering::Relaxed);
            return None;
        }
        Some(SubstitutedFace {
            program,
            identity,
            reason,
        })
    }
}

fn read_program(path: &Path) -> Option<Vec<u8>> {
    let file = File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() || file.metadata().ok()?.len() > MAX_FONT_BYTES as u64 {
        return None;
    }
    let mut bytes = Vec::new();
    file.take((MAX_FONT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() <= MAX_FONT_BYTES).then_some(bytes)
}

fn decision(answer: Option<&SubstitutedFace>) -> Decision {
    answer.map(|face| {
        (
            PathBuf::from(&face.identity.origin),
            face.identity.face_index,
            face.identity.sha256.clone(),
            face.reason,
        )
    })
}

fn request_key(request: &FontRequest) -> String {
    format!(
        "{}|{}|{}|{}|{}|{:?}",
        normalize_name(&request.family),
        request.style.weight,
        u8::from(request.style.italic),
        request.flags.0,
        request
            .standard_face
            .map_or("", crate::standard14::Standard14::name),
        request.base_font,
    )
}

impl FontProvider for SystemFontProvider {
    fn primary_face(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        let key = request_key(request);
        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.primary.get(&key).cloned());
        if let Some(found) = cached {
            return self.recall(found);
        }
        let answer = self.resolve_primary(request);
        if let Ok(mut cache) = self.cache.lock() {
            if cache.primary.len() >= MAX_DECISIONS {
                cache.primary.clear();
            }
            cache.primary.insert(key, decision(answer.as_ref()));
        }
        answer
    }

    fn fallback_face(&self, request: &FontRequest, character: char) -> Option<SubstitutedFace> {
        let key = (request_key(request), character);
        let cached = self
            .cache
            .lock()
            .ok()
            .and_then(|cache| cache.coverage.get(&key).cloned());
        if let Some(found) = cached {
            return self.recall(found);
        }
        let covers = |program: &GlyphProgram| program.glyph_for_char(character).is_some();
        let answer = self
            .first_matching(
                &[request.family.as_str()],
                request.style,
                SubstitutionReason::ScriptCoverage,
                covers,
            )
            .or_else(|| {
                self.first_matching(
                    coverage_faces(character),
                    request.style,
                    SubstitutionReason::ScriptCoverage,
                    covers,
                )
            });
        if let Ok(mut cache) = self.cache.lock() {
            if cache.coverage.len() >= MAX_DECISIONS {
                cache.coverage.clear();
            }
            cache.coverage.insert(key, decision(answer.as_ref()));
        }
        answer
    }

    fn description(&self) -> String {
        self.index_manifest()
    }
}

impl SystemFontProvider {
    fn resolve_primary(&self, request: &FontRequest) -> Option<SubstitutedFace> {
        let normalized = normalize_name(&request.family);
        if !normalized.is_empty()
            && let Some(face) = self.best_in_family(&normalized, request.style)
            && let Some((program, identity)) = self.load(face)
        {
            return Some(SubstitutedFace {
                program,
                identity,
                reason: SubstitutionReason::ExactFamily,
            });
        }
        if let Some(face) = self.first_available(
            family_aliases(&normalized),
            request.style,
            SubstitutionReason::AliasedFamily,
        ) {
            return Some(face);
        }
        if let Some(face) = self.first_available(
            byte_family_aliases(&request.base_font),
            request.style,
            SubstitutionReason::AliasedFamily,
        ) {
            return Some(face);
        }
        if let Some(standard) = request.standard_face
            && let Some(face) = self.first_available(
                standard_face_aliases(standard),
                request.style,
                SubstitutionReason::StandardFace,
            )
        {
            return Some(face);
        }
        self.first_available(
            generic_faces(request.generic()),
            request.style,
            SubstitutionReason::Generic,
        )
    }
}

fn walk(directory: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut children = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            children.push(path);
            continue;
        }
        if is_font_file(&path) {
            out.push(path);
            if out.len() >= MAX_FILES {
                return;
            }
        }
    }
    children.sort();
    for child in children {
        walk(&child, depth + 1, out);
    }
}

fn is_font_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
}

fn read_faces(path: &Path) -> Vec<InstalledFace> {
    let Ok(mut file) = File::open(path) else {
        return Vec::new();
    };
    let Ok(metadata) = file.metadata() else {
        return Vec::new();
    };
    read_faces_from(&mut file, path, metadata.len())
}

fn read_faces_from(file: &mut (impl Read + Seek), path: &Path, size: u64) -> Vec<InstalledFace> {
    let mut header = [0_u8; 12];
    if file.read_exact(&mut header).is_err() {
        return Vec::new();
    }
    let tag = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    let directories: Vec<u64> = match tag {
        0x0001_0000 | 0x7472_7565 | 0x4f54_544f => vec![0],
        0x7474_6366 => {
            let count = u32::from_be_bytes([header[8], header[9], header[10], header[11]]);
            let count = count.min(MAX_FACES_PER_FILE);
            let mut offsets = Vec::with_capacity(count as usize);
            for index in 0..count {
                let mut offset = [0_u8; 4];
                if file
                    .seek(SeekFrom::Start(12 + u64::from(index) * 4))
                    .is_err()
                    || file.read_exact(&mut offset).is_err()
                {
                    break;
                }
                offsets.push(u64::from(u32::from_be_bytes(offset)));
            }
            offsets
        }
        _ => return Vec::new(),
    };
    let mut faces = Vec::new();
    for (index, directory) in directories.into_iter().enumerate() {
        let Ok(index) = u32::try_from(index) else {
            break;
        };
        if let Some(face) = read_one_face(file, path, size, directory, index) {
            faces.push(face);
        }
    }
    faces
}

const MAX_FACES_PER_FILE: u32 = 64;

fn read_one_face(
    file: &mut (impl Read + Seek),
    path: &Path,
    size: u64,
    directory: u64,
    index: u32,
) -> Option<InstalledFace> {
    let mut count_bytes = [0_u8; 2];
    file.seek(SeekFrom::Start(directory.checked_add(4)?)).ok()?;
    file.read_exact(&mut count_bytes).ok()?;
    let count = u16::from_be_bytes(count_bytes);
    if count == 0 || count > 512 {
        return None;
    }
    let mut records = vec![0_u8; usize::from(count) * 16];
    file.seek(SeekFrom::Start(directory.checked_add(12)?))
        .ok()?;
    file.read_exact(&mut records).ok()?;
    let mut name_table = None;
    let mut os2_table = None;
    let mut has_outlines = false;
    let mut has_cmap = false;
    for record in records.chunks_exact(16) {
        let tag = &record[..4];
        let start = u32::from_be_bytes([record[8], record[9], record[10], record[11]]);
        let length = u32::from_be_bytes([record[12], record[13], record[14], record[15]]);
        match tag {
            b"name" => name_table = Some((start, length)),
            b"OS/2" => os2_table = Some((start, length)),
            b"glyf" | b"CFF " => has_outlines = true,
            b"cmap" => has_cmap = true,
            _ => {}
        }
    }
    if !has_outlines || !has_cmap {
        return None;
    }
    let (name_start, name_length) = name_table?;
    let names = read_at(file, u64::from(name_start), name_length as usize)?;
    let family = name_record(&names, 16).or_else(|| name_record(&names, 1))?;
    let subfamily = name_record(&names, 17)
        .or_else(|| name_record(&names, 2))
        .unwrap_or_else(|| "Regular".to_owned());
    let mut style = style_from_subfamily(&subfamily);
    if let Some((start, length)) = os2_table
        && length >= 64
        && let Some(os2) = read_at(file, u64::from(start), 64)
    {
        let weight = u16::from_be_bytes([os2[4], os2[5]]);
        let selection = u16::from_be_bytes([os2[62], os2[63]]);
        if (100..=1000).contains(&weight) {
            style.weight = weight;
        }
        style.italic = selection & 0x0001 != 0 || style.italic;
    }
    Some(InstalledFace {
        path: path.to_path_buf(),
        index,
        family,
        subfamily,
        style,
        bytes: size,
        expected_sha256: None,
    })
}

fn read_at(file: &mut (impl Read + Seek), offset: u64, length: usize) -> Option<Vec<u8>> {
    if length > 1 << 20 {
        return None;
    }
    let mut buffer = vec![0_u8; length];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut buffer).ok()?;
    Some(buffer)
}

fn name_record(table: &[u8], wanted: u16) -> Option<String> {
    let count = u16::from_be_bytes([*table.get(2)?, *table.get(3)?]);
    let storage = usize::from(u16::from_be_bytes([*table.get(4)?, *table.get(5)?]));
    let mut best: Option<String> = None;
    for index in 0..usize::from(count) {
        let record = 6 + index * 12;
        let slice = table.get(record..record + 12)?;
        let platform = u16::from_be_bytes([slice[0], slice[1]]);
        if u16::from_be_bytes([slice[6], slice[7]]) != wanted {
            continue;
        }
        let length = usize::from(u16::from_be_bytes([slice[8], slice[9]]));
        let offset = usize::from(u16::from_be_bytes([slice[10], slice[11]]));
        let start = storage.checked_add(offset)?;
        let Some(text) = table.get(start..start.checked_add(length)?) else {
            continue;
        };
        let decoded: Option<String> = match platform {
            0 | 3 => text
                .chunks_exact(2)
                .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
                .map(|unit| {
                    u8::try_from(unit)
                        .ok()
                        .filter(|byte| byte.is_ascii_graphic() || *byte == b' ')
                        .map(char::from)
                })
                .collect(),
            1 => text
                .iter()
                .map(|byte| (byte.is_ascii_graphic() || *byte == b' ').then_some(char::from(*byte)))
                .collect(),
            _ => None,
        };
        let Some(decoded) = decoded else { continue };
        if decoded.is_empty() {
            continue;
        }
        if platform == 3 {
            return Some(decoded);
        }
        best = best.or(Some(decoded));
    }
    best
}

fn style_from_subfamily(subfamily: &str) -> FontStyle {
    let normalized = normalize_name(subfamily);
    let italic = normalized.contains("italic") || normalized.contains("oblique");
    let weight = if normalized.contains("extrabold") || normalized.contains("ultrabold") {
        800
    } else if normalized.contains("semibold") || normalized.contains("demibold") {
        600
    } else if normalized.contains("bold") {
        700
    } else if normalized.contains("black") || normalized.contains("heavy") {
        900
    } else if normalized.contains("extralight") || normalized.contains("ultralight") {
        200
    } else if normalized.contains("light") {
        300
    } else if normalized.contains("thin") {
        100
    } else if normalized.contains("medium") {
        500
    } else {
        400
    };
    FontStyle { weight, italic }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font(covers_a: bool) -> Vec<u8> {
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        let maxp = vec![0, 0, 0, 0, 0, 2];
        let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12, 0, 0, 1, 6, 0, 0];
        cmap.resize(274, 0);
        cmap[18 + 65] = u8::from(covers_a);
        let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"head", head),
            (b"maxp", maxp),
            (b"loca", vec![0; 6]),
            (b"glyf", vec![]),
            (b"cmap", cmap),
        ];
        let mut bytes = vec![0, 1, 0, 0, 0, 5, 0, 0, 0, 0, 0, 0];
        let mut offset = 12 + tables.len() * 16;
        for (tag, data) in &tables {
            bytes.extend_from_slice(*tag);
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&u32::try_from(offset).unwrap().to_be_bytes());
            bytes.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            offset += data.len();
        }
        for (_, data) in tables {
            bytes.extend(data);
        }
        bytes
    }

    struct TempTree(PathBuf);
    impl TempTree {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "panpdf-font-review-{}-{serial}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn face(&self, name: &str, covers_a: bool) -> InstalledFace {
            let path = self.0.join(format!("{name}.ttf"));
            let bytes = test_font(covers_a);
            std::fs::write(&path, &bytes).unwrap();
            InstalledFace {
                path,
                index: 0,
                family: name.to_owned(),
                subfamily: "Regular".to_owned(),
                style: FontStyle::default(),
                bytes: bytes.len() as u64,
                expected_sha256: None,
            }
        }
    }
    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn with_faces(faces: Vec<InstalledFace>) -> SystemFontProvider {
        let by_family = faces
            .iter()
            .enumerate()
            .map(|(i, f)| (normalize_name(&f.family), vec![i]))
            .collect();
        SystemFontProvider {
            faces,
            by_family,
            roots: vec![],
            skipped: 0,
            cache: Mutex::new(Cache::default()),
            changed_faces: AtomicUsize::new(0),
        }
    }

    #[test]
    fn a_fallback_takes_the_family_the_file_names_when_it_shows_the_character() {
        let tree = TempTree::new();
        let provider = with_faces(vec![
            tree.face("DejaVu Sans", true),
            tree.face("Requested", true),
        ]);
        let request = |family: &str| FontRequest {
            base_font: format!("BAAAAA+{family}").into_bytes(),
            family: family.to_owned(),
            style: FontStyle::default(),
            subtype: b"Type0".to_vec(),
            cid_subtype: None,
            registry: None,
            ordering: None,
            encoding: None,
            flags: crate::substitute::FontFlags(0),
            italic_angle: None,
            ascent: None,
            descent: None,
            standard_face: None,
            program: crate::substitute::ProgramEvidence::NotEmbedded,
        };
        let named = provider.fallback_face(&request("Requested"), 'A').unwrap();
        assert_eq!(named.identity.family, "Requested");
        let absent = provider.fallback_face(&request("Absent"), 'A').unwrap();
        assert_eq!(absent.identity.family, "DejaVu Sans");
    }

    #[test]
    fn review_coverage_continues_past_an_installed_face_missing_the_character() {
        let tree = TempTree::new();
        let provider = with_faces(vec![tree.face("First", false), tree.face("Second", true)]);
        let first = provider
            .first_available(
                &["First"],
                FontStyle::default(),
                SubstitutionReason::Generic,
            )
            .unwrap();
        assert_eq!(first.program.glyph_for_char('A'), None, "negative control");
        let found = provider
            .first_matching(
                &["First", "Second"],
                FontStyle::default(),
                SubstitutionReason::ScriptCoverage,
                |p| p.glyph_for_char('A').is_some(),
            )
            .unwrap();
        assert_eq!(found.identity.family, "Second");
        assert_eq!(found.program.glyph_for_char('A'), Some(1));
    }

    #[test]
    fn review_decision_cache_does_not_keep_evicted_programs_alive() {
        let tree = TempTree::new();
        let faces = (0..=MAX_CACHED_FACES)
            .map(|i| tree.face(&format!("Face{i}"), true))
            .collect();
        let provider = with_faces(faces);
        let answer = provider
            .first_available(
                &["Face0"],
                FontStyle::default(),
                SubstitutionReason::Generic,
            )
            .unwrap();
        let weak = Arc::downgrade(&answer.program);
        let saved = decision(Some(&answer));
        drop(answer);
        provider
            .cache
            .lock()
            .unwrap()
            .primary
            .insert("request".to_owned(), saved.clone());
        for face in &provider.faces[1..] {
            drop(provider.load(face).unwrap());
        }
        assert!(
            weak.upgrade().is_none(),
            "an evicted program is still owned by a decision"
        );
        let reloaded = provider.recall(saved).unwrap();
        assert_eq!(
            reloaded.program.glyph_for_char('A'),
            Some(1),
            "eviction must not change resolution"
        );
        assert_eq!(
            provider.cache.lock().unwrap().loaded.len(),
            MAX_CACHED_FACES
        );
    }

    #[test]
    fn review_a_font_file_replaced_under_the_process_is_refused_rather_than_drawn() {
        let tree = TempTree::new();
        let faces = (0..=MAX_CACHED_FACES)
            .map(|index| tree.face(&format!("Face{index}"), true))
            .collect();
        let provider = with_faces(faces);
        let answer = provider
            .first_available(
                &["Face0"],
                FontStyle::default(),
                SubstitutionReason::Generic,
            )
            .expect("the face loads");
        let saved = decision(Some(&answer));
        let path = provider.faces[0].path.clone();
        drop(answer);
        for face in &provider.faces[1..] {
            drop(provider.load(face).expect("the other faces load"));
        }
        let recalled = provider.recall(saved.clone()).expect("unchanged file");
        assert_eq!(recalled.program.glyph_for_char('A'), Some(1));
        assert_eq!(provider.changed_faces_refused(), 0);

        std::fs::write(&path, test_font(false)).expect("the file is replaced");
        provider.cache.lock().expect("cache").loaded.clear();
        provider.cache.lock().expect("cache").order.clear();
        assert!(
            provider.recall(saved).is_none(),
            "a face whose bytes changed must not answer under its old identity"
        );
        assert_eq!(provider.changed_faces_refused(), 1);
        assert!(provider.index_manifest().contains("their file changed"));
    }

    #[test]
    fn review_a_collection_offers_every_face_it_holds() {
        let tree = TempTree::new();
        let path = tree.0.join("pair.ttc");
        std::fs::write(
            &path,
            collection(&[("Collection One", true), ("Collection Two", false)]),
        )
        .expect("the collection is written");

        let faces = read_faces(&path);
        assert_eq!(faces.len(), 2, "a two-face collection offers two faces");
        assert_eq!(faces[0].index, 0);
        assert_eq!(faces[1].index, 1);
        assert_eq!(faces[0].path, faces[1].path, "one file, two faces");
        assert_eq!(faces[0].family, "Collection One");
        assert_eq!(faces[1].family, "Collection Two");

        let provider = with_faces(faces);
        let one = provider.load(&provider.faces[0]).expect("face 0 loads");
        let other = provider.load(&provider.faces[1]).expect("face 1 loads");
        assert_eq!(one.0.glyph_for_char('A'), Some(1));
        assert_eq!(other.0.glyph_for_char('A'), None);
        assert_eq!(
            one.1.sha256, other.1.sha256,
            "the hash is the file's, so the two faces share it"
        );
        assert_ne!(
            one.1.face_index, other.1.face_index,
            "the index is what tells them apart"
        );
    }

    #[test]
    fn review_a_face_index_past_the_end_of_a_collection_is_refused() {
        let bytes = collection(&[("Collection One", true), ("Collection Two", false)]);
        assert_eq!(
            crate::truetype::TrueTypeFont::face_count(&bytes).expect("a collection"),
            2
        );
        assert!(crate::glyph::GlyphProgram::parse_face(bytes.clone(), 1).is_ok());
        assert!(crate::glyph::GlyphProgram::parse_face(bytes, 2).is_err());
        let plain = test_font(true);
        assert_eq!(
            crate::truetype::TrueTypeFont::face_count(&plain).expect("an sfnt"),
            1
        );
        assert!(crate::glyph::GlyphProgram::parse_face(plain, 1).is_err());
    }

    fn named_font(family: &str, covers_a: bool, base: usize) -> Vec<u8> {
        let mut head = vec![0; 54];
        head[18..20].copy_from_slice(&1000_u16.to_be_bytes());
        let maxp = vec![0, 0, 0, 0, 0, 2];
        let mut cmap = vec![0, 0, 0, 1, 0, 3, 0, 1, 0, 0, 0, 12, 0, 0, 1, 6, 0, 0];
        cmap.resize(274, 0);
        cmap[18 + 65] = u8::from(covers_a);
        let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"cmap", cmap),
            (b"glyf", vec![]),
            (b"head", head),
            (b"loca", vec![0; 6]),
            (b"maxp", maxp),
            (b"name", name_table(family)),
        ];
        let mut bytes = vec![0, 1, 0, 0];
        bytes.extend_from_slice(&u16::try_from(tables.len()).unwrap().to_be_bytes());
        bytes.extend_from_slice(&[0; 6]);
        let mut offset = base + 12 + tables.len() * 16;
        for (tag, data) in &tables {
            bytes.extend_from_slice(*tag);
            bytes.extend_from_slice(&[0; 4]);
            bytes.extend_from_slice(&u32::try_from(offset).unwrap().to_be_bytes());
            bytes.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            offset += data.len();
        }
        for (_, data) in tables {
            bytes.extend(data);
        }
        bytes
    }

    fn name_table(family: &str) -> Vec<u8> {
        let value: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut out = Vec::new();
        out.extend_from_slice(&0_u16.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        out.extend_from_slice(&(6 + 12_u16).to_be_bytes());
        out.extend_from_slice(&3_u16.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        out.extend_from_slice(&0x0409_u16.to_be_bytes());
        out.extend_from_slice(&1_u16.to_be_bytes());
        out.extend_from_slice(&u16::try_from(value.len()).unwrap().to_be_bytes());
        out.extend_from_slice(&0_u16.to_be_bytes());
        out.extend_from_slice(&value);
        out
    }

    fn collection(families: &[(&str, bool)]) -> Vec<u8> {
        let count = u32::try_from(families.len()).expect("face count");
        let header = 12 + 4 * families.len();
        let sizes: Vec<usize> = families
            .iter()
            .map(|(family, covers)| named_font(family, *covers, 0).len())
            .collect();
        let mut offsets = Vec::new();
        let mut at = header;
        for size in &sizes {
            offsets.push(at);
            at += size;
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"ttcf");
        out.extend_from_slice(&0x0001_0000_u32.to_be_bytes());
        out.extend_from_slice(&count.to_be_bytes());
        for offset in &offsets {
            out.extend_from_slice(&u32::try_from(*offset).expect("offset").to_be_bytes());
        }
        for ((family, covers), offset) in families.iter().zip(&offsets) {
            out.extend_from_slice(&named_font(family, *covers, *offset));
        }
        out
    }

    #[test]
    fn package_replacement_is_refused_before_first_load_and_after_eviction() {
        for warm in [false, true] {
            let tree = TempTree::new();
            let path = tree.0.join("package.ttf");
            let bytes = named_font("Pinned", true, 0);
            std::fs::write(&path, &bytes).unwrap();
            let (provider, problems) = SystemFontProvider::from_package(&[PackagedFace {
                path: path.clone(),
                face_index: 0,
                sha256: crate::sha256::hex(&bytes),
            }]);
            assert!(problems.is_empty());
            if warm {
                assert!(provider.load(&provider.faces[0]).is_some());
                *provider.cache.lock().unwrap() = Cache::default();
            }
            std::fs::write(&path, named_font("Replacement", false, 0)).unwrap();
            assert!(provider.load(&provider.faces[0]).is_none());
            assert_eq!(provider.changed_faces_refused(), 1);
        }
    }

    #[test]
    fn review_a_packaged_provider_uses_the_manifest_order_and_checks_every_hash() {
        let tree = TempTree::new();
        let write = |name: &str, family: &str, covers: bool| {
            let path = tree.0.join(name);
            let bytes = named_font(family, covers, 0);
            std::fs::write(&path, &bytes).expect("the face is written");
            PackagedFace {
                path,
                face_index: 0,
                sha256: crate::sha256::hex(&bytes),
            }
        };
        let first = write("first.ttf", "Packaged", true);
        let second = write("second.ttf", "Packaged", false);
        let (provider, problems) =
            SystemFontProvider::from_package(&[first.clone(), second.clone()]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(provider.faces().len(), 2);
        let chosen = provider
            .first_available(
                &["Packaged"],
                FontStyle::default(),
                SubstitutionReason::Generic,
            )
            .expect("the family resolves");
        assert_eq!(
            chosen.program.glyph_for_char('A'),
            Some(1),
            "the manifest's first entry answered"
        );
        assert_eq!(chosen.identity.family, "Packaged");
        assert_eq!(chosen.identity.sha256, first.sha256);

        let (reversed, _) = SystemFontProvider::from_package(&[second, first.clone()]);
        assert_eq!(
            reversed
                .first_available(
                    &["Packaged"],
                    FontStyle::default(),
                    SubstitutionReason::Generic,
                )
                .expect("the family resolves")
                .program
                .glyph_for_char('A'),
            None
        );

        let mut wrong = first;
        wrong.sha256 = "0".repeat(64);
        let (refused, problems) = SystemFontProvider::from_package(&[wrong]);
        assert!(refused.faces().is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("manifest 0000"), "{}", problems[0]);
        assert!(refused.index_manifest().starts_with("packaged;"));
    }

    #[test]
    fn review_a_packaged_provider_ignores_a_face_its_manifest_does_not_name() {
        let tree = TempTree::new();
        let named = tree.0.join("named.ttf");
        let bytes = named_font("Named", true, 0);
        std::fs::write(&named, &bytes).expect("the named face is written");
        let unnamed = tree.0.join("unnamed.ttf");
        std::fs::write(&unnamed, named_font("Unnamed", true, 0)).expect("the other face");

        let (provider, problems) = SystemFontProvider::from_package(&[PackagedFace {
            path: named,
            face_index: 0,
            sha256: crate::sha256::hex(&bytes),
        }]);
        assert!(problems.is_empty());
        assert_eq!(provider.faces().len(), 1);
        assert!(
            provider
                .first_available(
                    &["Unnamed"],
                    FontStyle::default(),
                    SubstitutionReason::Generic,
                )
                .is_none(),
            "a face beside the package but not in it must not be reachable"
        );
    }

    #[test]
    fn review_oversized_font_is_refused_before_reading_its_body() {
        let tree = TempTree::new();
        let path = tree.0.join("oversized.ttf");
        File::create(&path)
            .unwrap()
            .set_len(MAX_FONT_BYTES as u64 + 1)
            .unwrap();
        assert!(read_program(&path).is_none());
        let small = tree.face("Small", true);
        assert!(read_program(&small.path).is_some(), "positive control");
    }

    #[test]
    fn a_missing_root_is_not_an_error() {
        let provider = SystemFontProvider::discover_in(&[PathBuf::from("/nonexistent-font-root")]);
        assert!(provider.faces().is_empty());
        assert_eq!(provider.skipped_files(), 0);
    }

    #[test]
    fn subfamily_styles_are_read() {
        assert_eq!(
            style_from_subfamily("Bold Italic"),
            FontStyle {
                weight: 700,
                italic: true
            }
        );
        assert_eq!(style_from_subfamily("Regular"), FontStyle::default());
        assert!(style_from_subfamily("Oblique").italic);
        assert_eq!(style_from_subfamily("SemiBold").weight, 600);
    }
}
