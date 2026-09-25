#![forbid(unsafe_code)]

use std::fmt;

mod bignum;
mod blocks;
mod cms;
mod der;
mod digest;
mod protect;
mod rsa;
mod seeded;
mod sha1;
mod sha2;
mod trust;
mod x509;

pub use cms::{Checked, Integrity, Trust, Who, check as check_signature};
pub use protect::{Allowed, MadeProtection, Wanted, make_protection, random_identifier};
pub use seeded::{SEED_BYTES, seed_random};
pub use x509::Moment;

use aes::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest, Md5};
use pdf_bytes::ByteStore;
use pdf_syntax::{
    DictionaryEntry, NumberKind, Object, ObjectKind, Reference, ResolveError, ResolveLimits,
    RevisionChain, RevisionIndex, StreamDecodeError, StreamObject, StringDecodeError,
    decode_stream_bytes, decode_string,
};
use rc4::{KeyInit, Rc4, StreamCipher};
use zeroize::Zeroizing;

const PASSWORD_PADDING: [u8; 32] = [
    0x28, 0xbf, 0x4e, 0x5e, 0x4e, 0x75, 0x8a, 0x41, 0x64, 0x00, 0x4e, 0x56, 0xff, 0xfa, 0x01, 0x08,
    0x2e, 0x2e, 0x00, 0xb6, 0xd0, 0x68, 0x3e, 0x80, 0x2f, 0x0c, 0xa9, 0xfe, 0x64, 0x53, 0x69, 0x7a,
];

const MAX_SECURITY_STRING_BYTES: usize = 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessLevel {
    Unauthenticated,
    User,
    Owner,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CipherMethod {
    Identity,
    Rc4,
    Aes128,
    Aes256,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrintAllowance {
    Refused,
    Degraded,
    Faithful,
}

impl PrintAllowance {
    #[must_use]
    pub const fn of(revision: u8, permissions: i32) -> Self {
        const PRINT: i32 = 1 << 2;
        const FAITHFUL: i32 = 1 << 11;
        if permissions & PRINT == 0 {
            Self::Refused
        } else if revision >= 3 && permissions & FAITHFUL == 0 {
            Self::Degraded
        } else {
            Self::Faithful
        }
    }
}

pub struct AuthenticatedSecurity {
    version: u8,
    revision: u8,
    access: AccessLevel,
    permissions: i32,
    encrypt_metadata: bool,
    stream_method: CipherMethod,
    string_method: CipherMethod,
    file_key: Zeroizing<Vec<u8>>,
}

impl AuthenticatedSecurity {
    #[must_use]
    pub const fn may_modify_content(&self) -> bool {
        matches!(self.access, AccessLevel::Owner) || self.permissions & 8 != 0
    }

    #[must_use]
    pub const fn print_allowance(&self) -> PrintAllowance {
        if matches!(self.access, AccessLevel::Owner) {
            return PrintAllowance::Faithful;
        }
        PrintAllowance::of(self.revision, self.permissions)
    }

    #[must_use]
    pub const fn version(&self) -> u8 {
        self.version
    }

    #[must_use]
    pub const fn revision(&self) -> u8 {
        self.revision
    }

    #[must_use]
    pub const fn access_level(&self) -> AccessLevel {
        self.access
    }

    #[must_use]
    pub const fn permissions(&self) -> i32 {
        self.permissions
    }

    #[must_use]
    pub const fn encrypt_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    #[must_use]
    pub const fn stream_method(&self) -> CipherMethod {
        self.stream_method
    }

    #[must_use]
    pub const fn string_method(&self) -> CipherMethod {
        self.string_method
    }

    pub fn decrypt_string(
        &self,
        reference: Reference,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        self.decrypt(reference, self.string_method, encrypted)
    }

    pub fn decrypt_stream(
        &self,
        reference: Reference,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        self.decrypt(reference, self.stream_method, encrypted)
    }

    pub fn encrypt_stream(
        &self,
        reference: Reference,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        self.encrypt(reference, self.stream_method, plaintext)
    }

    pub fn encrypt_string(
        &self,
        reference: Reference,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        self.encrypt(reference, self.string_method, plaintext)
    }

    fn encrypt(
        &self,
        reference: Reference,
        method: CipherMethod,
        plaintext: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        match method {
            CipherMethod::Identity => Ok(plaintext.to_vec()),
            CipherMethod::Rc4 => {
                let key = object_key(&self.file_key, reference, false);
                rc4_process(plaintext, &key)
            }
            CipherMethod::Aes128 => {
                let key = object_key(&self.file_key, reference, true);
                encrypt_aes_cbc::<aes::Aes128>(plaintext, &key, &random_iv()?)
            }
            CipherMethod::Aes256 => {
                encrypt_aes_cbc::<aes::Aes256>(plaintext, &self.file_key, &random_iv()?)
            }
        }
    }

    pub fn decrypt_and_decode_stream(
        &self,
        source: &ByteStore,
        reference: Reference,
        dictionary: &[DictionaryEntry],
        stream: &StreamObject,
        max_decoded_bytes: usize,
    ) -> Result<Vec<u8>, SecurityError> {
        let encrypted = source
            .resolve(stream.data_span())
            .map_err(|_| SecurityError::new(SecurityErrorKind::StreamSourceSpanFailure))?;
        let encoded = self.decrypt_stream(reference, encrypted)?;
        decode_stream_bytes(
            source,
            dictionary,
            &encoded,
            stream.data_span().start(),
            max_decoded_bytes,
        )
        .map_err(|error| SecurityError::new(SecurityErrorKind::StreamDecode(error)))
    }

    fn decrypt(
        &self,
        reference: Reference,
        method: CipherMethod,
        encrypted: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        match method {
            CipherMethod::Identity => Ok(encrypted.to_vec()),
            CipherMethod::Rc4 => {
                let key = object_key(&self.file_key, reference, false);
                rc4_process(encrypted, &key)
            }
            CipherMethod::Aes128 => {
                let key = object_key(&self.file_key, reference, true);
                decrypt_aes_cbc::<aes::Aes128>(encrypted, &key)
            }
            CipherMethod::Aes256 => decrypt_aes_cbc::<aes::Aes256>(encrypted, &self.file_key),
        }
    }
}

impl fmt::Debug for AuthenticatedSecurity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AuthenticatedSecurity")
            .field("version", &self.version)
            .field("revision", &self.revision)
            .field("access", &self.access)
            .field("permissions", &self.permissions)
            .field("encrypt_metadata", &self.encrypt_metadata)
            .field("stream_method", &self.stream_method)
            .field("string_method", &self.string_method)
            .field("file_key", &"[REDACTED]")
            .finish()
    }
}

pub fn authenticate_standard_password(
    source: &ByteStore,
    chain: &RevisionChain,
    index: &RevisionIndex,
    password: &[u8],
    resolve_limits: ResolveLimits,
) -> Result<AuthenticatedSecurity, SecurityError> {
    let trailer = chain
        .revisions()
        .first()
        .ok_or_else(|| SecurityError::new(SecurityErrorKind::MissingTrailer))?
        .trailer();
    let ObjectKind::Dictionary(trailer_entries) = trailer.kind() else {
        return Err(SecurityError::new(SecurityErrorKind::InvalidTrailer));
    };
    let id = parse_file_identifier(source, trailer_entries)?;
    let encrypt = required_entry(source, trailer_entries, b"/Encrypt", "Encrypt")?;
    let parameters = match encrypt.kind() {
        ObjectKind::Dictionary(_) => parse_parameters(source, encrypt, &id)?,
        ObjectKind::Reference(reference) => {
            let resolved = index
                .resolve_object(source, *reference, resolve_limits)
                .map_err(|error| SecurityError::new(SecurityErrorKind::Resolve(error)))?;
            if resolved.is_compressed() || resolved.stream().is_some() {
                return Err(SecurityError::new(
                    SecurityErrorKind::InvalidEncryptionDictionary,
                ));
            }
            parse_parameters(resolved.source(), resolved.value(), &id)?
        }
        _ => {
            return Err(SecurityError::new(
                SecurityErrorKind::InvalidEncryptionDictionary,
            ));
        }
    };

    authenticate(&parameters, password)
}

#[derive(Debug)]
struct Parameters {
    version: u8,
    revision: u8,
    key_bytes: usize,
    owner: Vec<u8>,
    user: Vec<u8>,
    owner_key: Vec<u8>,
    user_key: Vec<u8>,
    permissions: i32,
    file_id: Vec<u8>,
    encrypt_metadata: bool,
    stream_method: CipherMethod,
    string_method: CipherMethod,
}

fn parse_file_identifier(
    source: &ByteStore,
    trailer: &[DictionaryEntry],
) -> Result<Vec<u8>, SecurityError> {
    let id = required_entry(source, trailer, b"/ID", "ID")?;
    let ObjectKind::Array(values) = id.kind() else {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry("ID")));
    };
    if values.len() != 2 {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry("ID")));
    }
    decode_string(source, &values[0], MAX_SECURITY_STRING_BYTES)
        .map_err(|error| SecurityError::new(SecurityErrorKind::String(error)))
}

fn parse_parameters(
    source: &ByteStore,
    dictionary: &Object,
    file_id: &[u8],
) -> Result<Parameters, SecurityError> {
    let ObjectKind::Dictionary(entries) = dictionary.kind() else {
        return Err(SecurityError::new(
            SecurityErrorKind::InvalidEncryptionDictionary,
        ));
    };
    let filter = required_entry(source, entries, b"/Filter", "Filter")?;
    if !filter.name_equals(source, b"/Standard") {
        return Err(SecurityError::new(SecurityErrorKind::UnsupportedHandler));
    }
    if optional_entry(source, entries, b"/SubFilter")?.is_some() {
        return Err(SecurityError::new(SecurityErrorKind::UnsupportedSubFilter));
    }

    let version = u8::try_from(required_integer(source, entries, b"/V", "V")?)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry("V")))?;
    let revision = u8::try_from(required_integer(source, entries, b"/R", "R")?)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry("R")))?;
    if !matches!((version, revision), (1, 2) | (2, 3) | (4, 4) | (5, 5 | 6)) {
        return Err(SecurityError::new(SecurityErrorKind::UnsupportedRevision {
            version,
            revision,
        }));
    }

    let key_bits = match optional_integer(source, entries, b"/Length", "Length")? {
        Some(value) => usize::try_from(value)
            .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry("Length")))?,
        None if version == 1 || version == 2 => 40,
        None if version == 5 => 256,
        None => 128,
    };
    if key_bits % 8 != 0
        || !(40..=256).contains(&key_bits)
        || (version == 1 && key_bits != 40)
        || (version == 4 && key_bits != 128)
        || (version == 5 && key_bits != 256)
        || (version < 5 && key_bits > 128)
    {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry(
            "Length",
        )));
    }

    let mut owner = required_string(source, entries, b"/O", "O")?;
    let mut user = required_string(source, entries, b"/U", "U")?;
    let wanted = if version == 5 { 48 } else { 32 };
    if owner.len() < wanted || user.len() < wanted {
        return Err(SecurityError::new(
            SecurityErrorKind::InvalidOwnerUserLength,
        ));
    }
    if version < 5 && (owner.len() != 32 || user.len() != 32) {
        return Err(SecurityError::new(
            SecurityErrorKind::InvalidOwnerUserLength,
        ));
    }
    owner.truncate(wanted);
    user.truncate(wanted);
    let (owner_key, user_key) = if version == 5 {
        let owner_key = required_string(source, entries, b"/OE", "OE")?;
        let user_key = required_string(source, entries, b"/UE", "UE")?;
        if owner_key.len() != 32 || user_key.len() != 32 {
            return Err(SecurityError::new(SecurityErrorKind::InvalidEntry("UE")));
        }
        (owner_key, user_key)
    } else {
        (Vec::new(), Vec::new())
    };
    let permissions = permission_bits(required_integer(source, entries, b"/P", "P")?)?;
    validate_permissions(revision, permissions)?;
    let encrypt_metadata = optional_boolean(source, entries, b"/EncryptMetadata")?.unwrap_or(true);
    let (stream_method, string_method) = if version == 4 || version == 5 {
        (
            parse_crypt_filter(source, entries, b"/StmF")?,
            parse_crypt_filter(source, entries, b"/StrF")?,
        )
    } else {
        (CipherMethod::Rc4, CipherMethod::Rc4)
    };

    Ok(Parameters {
        version,
        revision,
        key_bytes: key_bits / 8,
        owner,
        user,
        owner_key,
        user_key,
        permissions,
        file_id: file_id.to_vec(),
        encrypt_metadata,
        stream_method,
        string_method,
    })
}

fn permission_bits(value: i64) -> Result<i32, SecurityError> {
    if let Ok(signed) = i32::try_from(value) {
        return Ok(signed);
    }
    u32::try_from(value)
        .map(|bits| i32::from_ne_bytes(bits.to_ne_bytes()))
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry("P")))
}

fn validate_permissions(revision: u8, permissions: i32) -> Result<(), SecurityError> {
    let bits = u32::from_ne_bytes(permissions.to_ne_bytes());
    let required_zeros = 0b11_u32;
    let required_ones = if revision == 2 {
        0xffff_ffc0
    } else {
        0xffff_f0c0
    };
    if bits & required_zeros != 0 || bits & required_ones != required_ones {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry("P")));
    }
    Ok(())
}

fn parse_crypt_filter(
    source: &ByteStore,
    encryption: &[DictionaryEntry],
    selector: &[u8],
) -> Result<CipherMethod, SecurityError> {
    let Some(selected) = optional_entry(source, encryption, selector)? else {
        return Ok(CipherMethod::Identity);
    };
    if selected.name_equals(source, b"/Identity") {
        return Ok(CipherMethod::Identity);
    }
    if !selected.name_equals(source, b"/StdCF") {
        return Err(SecurityError::new(
            SecurityErrorKind::UnsupportedCryptFilter,
        ));
    }
    let filters = required_entry(source, encryption, b"/CF", "CF")?;
    let ObjectKind::Dictionary(filter_entries) = filters.kind() else {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry("CF")));
    };
    let standard = required_entry(source, filter_entries, b"/StdCF", "StdCF")?;
    let ObjectKind::Dictionary(standard_entries) = standard.kind() else {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry("StdCF")));
    };
    if let Some(event) = optional_entry(source, standard_entries, b"/AuthEvent")?
        && !event.name_equals(source, b"/DocOpen")
    {
        return Err(SecurityError::new(
            SecurityErrorKind::UnsupportedCryptFilter,
        ));
    }
    if let Some(length) = optional_integer(source, standard_entries, b"/Length", "CF.Length")?
        && !matches!(length, 16 | 32 | 128 | 256)
    {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry(
            "CF.Length",
        )));
    }
    let method = required_entry(source, standard_entries, b"/CFM", "CFM")?;
    if method.name_equals(source, b"/V2") {
        Ok(CipherMethod::Rc4)
    } else if method.name_equals(source, b"/AESV2") {
        Ok(CipherMethod::Aes128)
    } else if method.name_equals(source, b"/AESV3") {
        Ok(CipherMethod::Aes256)
    } else {
        Err(SecurityError::new(
            SecurityErrorKind::UnsupportedCryptFilter,
        ))
    }
}

fn authenticate(
    parameters: &Parameters,
    password: &[u8],
) -> Result<AuthenticatedSecurity, SecurityError> {
    let session = if parameters.version == 5 {
        authenticate_two_point_zero(parameters, password)
    } else {
        authenticate_legacy(parameters, password)
    };
    match session {
        Ok(session) => Ok(session),
        Err(error)
            if error.kind() == SecurityErrorKind::InvalidPassword
                && parameters.stream_method == CipherMethod::Identity
                && parameters.string_method == CipherMethod::Identity =>
        {
            Ok(AuthenticatedSecurity {
                version: parameters.version,
                revision: parameters.revision,
                access: AccessLevel::Unauthenticated,
                permissions: parameters.permissions,
                encrypt_metadata: parameters.encrypt_metadata,
                stream_method: CipherMethod::Identity,
                string_method: CipherMethod::Identity,
                file_key: Zeroizing::new(Vec::new()),
            })
        }
        Err(error) => Err(error),
    }
}

fn authenticate_legacy(
    parameters: &Parameters,
    password: &[u8],
) -> Result<AuthenticatedSecurity, SecurityError> {
    let (access, file_key) = if let Some(key) = authenticate_owner_password(parameters, password)? {
        (AccessLevel::Owner, key)
    } else if let Some(key) = authenticate_user_password(parameters, password)? {
        (AccessLevel::User, key)
    } else {
        return Err(SecurityError::new(SecurityErrorKind::InvalidPassword));
    };
    Ok(AuthenticatedSecurity {
        version: parameters.version,
        revision: parameters.revision,
        access,
        permissions: parameters.permissions,
        encrypt_metadata: parameters.encrypt_metadata,
        stream_method: parameters.stream_method,
        string_method: parameters.string_method,
        file_key: Zeroizing::new(file_key),
    })
}

fn authenticate_two_point_zero(
    parameters: &Parameters,
    password: &[u8],
) -> Result<AuthenticatedSecurity, SecurityError> {
    let password = &password[..password.len().min(127)];
    let user_validation = &parameters.user[32..40];
    let user_salt = &parameters.user[40..48];
    let owner_validation = &parameters.owner[32..40];
    let owner_salt = &parameters.owner[40..48];
    let user_data = &parameters.user[..48];

    let owner_hash =
        two_point_zero_hash(parameters.revision, password, owner_validation, user_data)?;
    if constant_time_eq(&owner_hash, &parameters.owner[..32]) {
        let intermediate =
            two_point_zero_hash(parameters.revision, password, owner_salt, user_data)?;
        let file_key = unwrap_file_key(&parameters.owner_key, &intermediate)?;
        return Ok(two_point_zero_session(
            parameters,
            AccessLevel::Owner,
            file_key,
        ));
    }
    let user_hash = two_point_zero_hash(parameters.revision, password, user_validation, &[])?;
    if constant_time_eq(&user_hash, &parameters.user[..32]) {
        let intermediate = two_point_zero_hash(parameters.revision, password, user_salt, &[])?;
        let file_key = unwrap_file_key(&parameters.user_key, &intermediate)?;
        return Ok(two_point_zero_session(
            parameters,
            AccessLevel::User,
            file_key,
        ));
    }
    Err(SecurityError::new(SecurityErrorKind::InvalidPassword))
}

fn two_point_zero_session(
    parameters: &Parameters,
    access: AccessLevel,
    file_key: Zeroizing<Vec<u8>>,
) -> AuthenticatedSecurity {
    AuthenticatedSecurity {
        version: parameters.version,
        revision: parameters.revision,
        access,
        permissions: parameters.permissions,
        encrypt_metadata: parameters.encrypt_metadata,
        stream_method: parameters.stream_method,
        string_method: parameters.string_method,
        file_key,
    }
}

fn unwrap_file_key(wrapped: &[u8], key: &[u8]) -> Result<Zeroizing<Vec<u8>>, SecurityError> {
    use aes::cipher::BlockModeDecrypt;

    if wrapped.len() != 32 {
        return Err(SecurityError::new(SecurityErrorKind::MalformedCiphertext));
    }
    let mut blocks = [
        aes::cipher::Array::from([0u8; 16]),
        aes::cipher::Array::from([0u8; 16]),
    ];
    blocks[0].copy_from_slice(&wrapped[..16]);
    blocks[1].copy_from_slice(&wrapped[16..]);
    cbc::Decryptor::<aes::Aes256>::new_from_slices(key, &[0u8; 16])
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?
        .decrypt_blocks(&mut blocks);
    let mut out = Zeroizing::new(Vec::with_capacity(32));
    out.extend_from_slice(&blocks[0]);
    out.extend_from_slice(&blocks[1]);
    Ok(out)
}

fn two_point_zero_hash(
    revision: u8,
    password: &[u8],
    salt: &[u8],
    user_data: &[u8],
) -> Result<Zeroizing<Vec<u8>>, SecurityError> {
    use aes::cipher::BlockModeEncrypt;

    let mut input = Zeroizing::new(Vec::with_capacity(
        password.len() + salt.len() + user_data.len(),
    ));
    input.extend_from_slice(password);
    input.extend_from_slice(salt);
    input.extend_from_slice(user_data);
    let mut digest = Zeroizing::new(sha2::sha256(&input).to_vec());
    if revision < 6 {
        return Ok(digest);
    }

    for round in 0u32.. {
        let mut block = Zeroizing::new(Vec::with_capacity(
            (password.len() + digest.len() + user_data.len()) * 64,
        ));
        for _ in 0..64 {
            block.extend_from_slice(password);
            block.extend_from_slice(&digest);
            block.extend_from_slice(user_data);
        }
        if !block.len().is_multiple_of(16) {
            return Err(SecurityError::new(SecurityErrorKind::MalformedCiphertext));
        }
        let mut blocks: Vec<aes::cipher::Array<u8, aes::cipher::consts::U16>> = block
            .chunks_exact(16)
            .map(|chunk| {
                let mut array = aes::cipher::Array::from([0u8; 16]);
                array.copy_from_slice(chunk);
                array
            })
            .collect();
        cbc::Encryptor::<aes::Aes128>::new_from_slices(&digest[..16], &digest[16..32])
            .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?
            .encrypt_blocks(&mut blocks);
        let encrypted: Zeroizing<Vec<u8>> =
            Zeroizing::new(blocks.iter().flatten().copied().collect());
        let modulus: u32 = encrypted[..16]
            .iter()
            .map(|byte| u32::from(*byte))
            .sum::<u32>()
            % 3;
        *digest = match modulus {
            0 => sha2::sha256(&encrypted).to_vec(),
            1 => sha2::sha384(&encrypted).to_vec(),
            _ => sha2::sha512(&encrypted).to_vec(),
        };
        let last = u32::from(*encrypted.last().unwrap_or(&0));
        if round >= 63 && last <= round.saturating_sub(31) {
            break;
        }
    }
    digest.truncate(32);
    Ok(digest)
}

fn authenticate_user_password(
    parameters: &Parameters,
    password: &[u8],
) -> Result<Option<Vec<u8>>, SecurityError> {
    let key = compute_file_key(parameters, password);
    let computed = compute_user_value(parameters, &key)?;
    let compared = if parameters.revision == 2 { 32 } else { 16 };
    Ok(constant_time_eq(&computed[..compared], &parameters.user[..compared]).then_some(key))
}

fn authenticate_owner_password(
    parameters: &Parameters,
    password: &[u8],
) -> Result<Option<Vec<u8>>, SecurityError> {
    let mut digest = md5(&pad_password(password));
    if parameters.revision >= 3 {
        for _ in 0..50 {
            digest = md5(&digest);
        }
    }
    let owner_key = &digest[..parameters.key_bytes];
    let mut recovered = Zeroizing::new(parameters.owner.clone());
    if parameters.revision == 2 {
        *recovered = rc4_process(&recovered, owner_key)?;
    } else {
        for iteration in (0_u8..20).rev() {
            let key: Vec<u8> = owner_key.iter().map(|byte| byte ^ iteration).collect();
            *recovered = rc4_process(&recovered, &key)?;
        }
    }
    let Some(file_key) = authenticate_user_password(parameters, &recovered)? else {
        return Ok(None);
    };
    Ok(Some(file_key))
}

fn compute_file_key(parameters: &Parameters, password: &[u8]) -> Vec<u8> {
    let mut hasher = Md5::new();
    hasher.update(pad_password(password));
    hasher.update(&parameters.owner);
    hasher.update(parameters.permissions.to_le_bytes());
    hasher.update(&parameters.file_id);
    if parameters.revision >= 4 && !parameters.encrypt_metadata {
        hasher.update([0xff; 4]);
    }
    let mut digest: [u8; 16] = hasher.finalize().into();
    if parameters.revision >= 3 {
        for _ in 0..50 {
            digest = md5(&digest[..parameters.key_bytes]);
        }
    }
    digest[..parameters.key_bytes].to_vec()
}

fn compute_user_value(parameters: &Parameters, file_key: &[u8]) -> Result<Vec<u8>, SecurityError> {
    if parameters.revision == 2 {
        return rc4_process(&PASSWORD_PADDING, file_key);
    }
    let mut hasher = Md5::new();
    hasher.update(PASSWORD_PADDING);
    hasher.update(&parameters.file_id);
    let mut value = hasher.finalize().to_vec();
    for iteration in 0_u8..20 {
        let key: Vec<u8> = file_key.iter().map(|byte| byte ^ iteration).collect();
        value = rc4_process(&value, &key)?;
    }
    Ok(value)
}

fn pad_password(password: &[u8]) -> [u8; 32] {
    let mut padded = [0_u8; 32];
    let copied = password.len().min(32);
    padded[..copied].copy_from_slice(&password[..copied]);
    padded[copied..].copy_from_slice(&PASSWORD_PADDING[..32 - copied]);
    padded
}

fn object_key(file_key: &[u8], reference: Reference, aes: bool) -> Zeroizing<Vec<u8>> {
    let object = reference.object_number().to_le_bytes();
    let generation = reference.generation().to_le_bytes();
    let mut input = Zeroizing::new(Vec::with_capacity(file_key.len() + 9));
    input.extend_from_slice(file_key);
    input.extend_from_slice(&object[..3]);
    input.extend_from_slice(&generation);
    if aes {
        input.extend_from_slice(b"sAlT");
    }
    let digest = md5(&input);
    let length = if aes {
        16
    } else {
        (file_key.len() + 5).min(16)
    };
    Zeroizing::new(digest[..length].to_vec())
}

fn rc4_process(input: &[u8], key: &[u8]) -> Result<Vec<u8>, SecurityError> {
    let mut cipher = Rc4::new_from_slice(key)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?;
    let mut output = input.to_vec();
    cipher.apply_keystream(&mut output);
    Ok(output)
}

fn decrypt_aes_cbc<C>(input: &[u8], key: &[u8]) -> Result<Vec<u8>, SecurityError>
where
    C: aes::cipher::BlockCipherDecrypt + aes::cipher::KeyInit,
{
    let (iv, ciphertext) = input
        .split_at_checked(16)
        .filter(|(_, ciphertext)| !ciphertext.is_empty() && ciphertext.len().is_multiple_of(16))
        .ok_or_else(|| SecurityError::new(SecurityErrorKind::MalformedCiphertext))?;
    cbc::Decryptor::<C>::new_from_slices(key, iv)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?
        .decrypt_padded_vec::<Pkcs7>(ciphertext)
        .map_err(|_| SecurityError::new(SecurityErrorKind::MalformedCiphertext))
}

fn encrypt_aes_cbc<C>(plaintext: &[u8], key: &[u8], iv: &[u8; 16]) -> Result<Vec<u8>, SecurityError>
where
    C: aes::cipher::BlockCipherEncrypt + aes::cipher::KeyInit,
{
    let ciphertext = cbc::Encryptor::<C>::new_from_slices(key, iv)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?
        .encrypt_padded_vec::<Pkcs7>(plaintext);
    let mut out = Vec::with_capacity(16 + ciphertext.len());
    out.extend_from_slice(iv);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn random_iv() -> Result<[u8; 16], SecurityError> {
    random_bytes::<16>()
}

fn random_bytes<const N: usize>() -> Result<[u8; N], SecurityError> {
    random_bytes_from(read_os_random)
}

fn random_bytes_from<const N: usize>(os: fn(&mut [u8]) -> bool) -> Result<[u8; N], SecurityError> {
    let mut bytes = [0_u8; N];
    if os(&mut bytes) || seeded::draw(&mut bytes) {
        Ok(bytes)
    } else {
        Err(SecurityError::new(
            SecurityErrorKind::UnsupportedWriteCipher,
        ))
    }
}

fn read_os_random(bytes: &mut [u8]) -> bool {
    use std::io::Read;
    std::fs::File::open("/dev/urandom")
        .and_then(|mut source| source.read_exact(bytes))
        .is_ok()
}

fn md5(input: &[u8]) -> [u8; 16] {
    Md5::digest(input).into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn required_string(
    source: &ByteStore,
    entries: &[DictionaryEntry],
    key: &[u8],
    field: &'static str,
) -> Result<Vec<u8>, SecurityError> {
    let value = required_entry(source, entries, key, field)?;
    decode_string(source, value, MAX_SECURITY_STRING_BYTES)
        .map_err(|error| SecurityError::new(SecurityErrorKind::String(error)))
}

fn required_integer(
    source: &ByteStore,
    entries: &[DictionaryEntry],
    key: &[u8],
    field: &'static str,
) -> Result<i64, SecurityError> {
    optional_integer(source, entries, key, field)?
        .ok_or_else(|| SecurityError::new(SecurityErrorKind::MissingEntry(field)))
}

fn optional_integer(
    source: &ByteStore,
    entries: &[DictionaryEntry],
    key: &[u8],
    field: &'static str,
) -> Result<Option<i64>, SecurityError> {
    let Some(value) = optional_entry(source, entries, key)? else {
        return Ok(None);
    };
    if !matches!(value.kind(), ObjectKind::Number(NumberKind::Integer)) {
        return Err(SecurityError::new(SecurityErrorKind::InvalidEntry(field)));
    }
    let bytes = source
        .resolve(value.span())
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry(field)))?;
    let text = std::str::from_utf8(bytes)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry(field)))?;
    text.parse::<i64>()
        .map(Some)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidEntry(field)))
}

fn optional_boolean(
    source: &ByteStore,
    entries: &[DictionaryEntry],
    key: &[u8],
) -> Result<Option<bool>, SecurityError> {
    let Some(value) = optional_entry(source, entries, key)? else {
        return Ok(None);
    };
    match value.kind() {
        ObjectKind::Boolean(boolean) => Ok(Some(*boolean)),
        _ => Err(SecurityError::new(SecurityErrorKind::InvalidEntry(
            "EncryptMetadata",
        ))),
    }
}

fn required_entry<'a>(
    source: &ByteStore,
    entries: &'a [DictionaryEntry],
    key: &[u8],
    field: &'static str,
) -> Result<&'a Object, SecurityError> {
    optional_entry(source, entries, key)?
        .ok_or_else(|| SecurityError::new(SecurityErrorKind::MissingEntry(field)))
}

fn optional_entry<'a>(
    source: &ByteStore,
    entries: &'a [DictionaryEntry],
    key: &[u8],
) -> Result<Option<&'a Object>, SecurityError> {
    let mut matching = entries.iter().filter(|entry| entry.key_equals(source, key));
    let Some(first) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() {
        return Err(SecurityError::new(SecurityErrorKind::DuplicateEntry));
    }
    Ok(Some(first.value()))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SecurityError {
    kind: SecurityErrorKind,
}

impl SecurityError {
    const fn new(kind: SecurityErrorKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> SecurityErrorKind {
        self.kind
    }
}

impl fmt::Display for SecurityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)
    }
}

impl std::error::Error for SecurityError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SecurityErrorKind {
    MissingTrailer,
    InvalidTrailer,
    MissingEntry(&'static str),
    InvalidEntry(&'static str),
    DuplicateEntry,
    InvalidEncryptionDictionary,
    UnsupportedHandler,
    UnsupportedSubFilter,
    UnsupportedRevision { version: u8, revision: u8 },
    UnsupportedCryptFilter,
    InvalidOwnerUserLength,
    InvalidPassword,
    InvalidCipherKey,
    MalformedCiphertext,
    UnsupportedWriteCipher,
    WeakRandomSeed,
    StreamSourceSpanFailure,
    Resolve(ResolveError),
    StreamDecode(StreamDecodeError),
    String(StringDecodeError),
}

impl fmt::Display for SecurityErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEntry(field) => write!(formatter, "encryption data is missing /{field}"),
            Self::InvalidEntry(field) => write!(formatter, "encryption entry /{field} is invalid"),
            Self::UnsupportedRevision { version, revision } => write!(
                formatter,
                "Standard Security Handler V={version}, R={revision} is not supported"
            ),
            Self::Resolve(error) => write!(formatter, "encryption dictionary: {error}"),
            Self::StreamDecode(error) => write!(formatter, "decrypted stream: {error}"),
            Self::String(error) => write!(formatter, "encryption string: {error}"),
            _ => formatter.write_str(match self {
                Self::MissingTrailer => "document has no active trailer",
                Self::InvalidTrailer => "active trailer is not a dictionary",
                Self::DuplicateEntry => "encryption data repeats a dictionary entry",
                Self::InvalidEncryptionDictionary => "Encrypt is not a direct dictionary object",
                Self::UnsupportedHandler => "document does not use the Standard security handler",
                Self::UnsupportedSubFilter => "encryption SubFilter is not supported",
                Self::UnsupportedCryptFilter => "encryption crypt filter is not supported",
                Self::InvalidOwnerUserLength => "encryption O and U strings must be 32 bytes",
                Self::InvalidPassword => "password does not authenticate this document",
                Self::InvalidCipherKey => "encryption key has an invalid length",
                Self::MalformedCiphertext => "encrypted object data or AES padding is malformed",
                Self::UnsupportedWriteCipher => {
                    "writing with AES needs random bytes, and none could be read from the operating system and no seed was given"
                }
                Self::WeakRandomSeed => {
                    "a random seed must be at least 48 bytes that are not all the same"
                }
                Self::StreamSourceSpanFailure => {
                    "encrypted stream span does not belong to the source"
                }
                Self::MissingEntry(_)
                | Self::InvalidEntry(_)
                | Self::UnsupportedRevision { .. }
                | Self::Resolve(_)
                | Self::StreamDecode(_)
                | Self::String(_) => unreachable!(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        AccessLevel, AuthenticatedSecurity, CipherMethod, Parameters, SecurityError,
        SecurityErrorKind, authenticate, authenticate_standard_password,
        authenticate_user_password, object_key, pad_password, permission_bits, rc4_process,
    };
    use aes::cipher::{BlockModeEncrypt, KeyIvInit, block_padding::Pkcs7};
    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::{
        ObjectKind, ParseLimits, Reference, ResolveLimits, RevisionIndex, XrefLimits,
        parse_indirect_object_strict, parse_revision_chain_strict,
    };

    fn decode_hex(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("ASCII fixture");
                u8::from_str_radix(text, 16).expect("hex fixture")
            })
            .collect()
    }

    fn r4_parameters() -> Parameters {
        Parameters {
            owner_key: Vec::new(),
            user_key: Vec::new(),
            version: 4,
            revision: 4,
            key_bytes: 16,
            owner: decode_hex("36451bd39d753b7c1d10922c28e6665aa4f3353fb0348b536893e3b1db5c579b"),
            user: decode_hex("32c27288b9ec6a4fab94e6188828595c0122456a91bae5134273a6db134c87c4"),
            permissions: -4,
            file_id: decode_hex("66d36a30a97e0f16f39955c6221e0c2a"),
            encrypt_metadata: true,
            stream_method: CipherMethod::Aes128,
            string_method: CipherMethod::Aes128,
        }
    }

    fn r3_parameters() -> Parameters {
        Parameters {
            owner_key: Vec::new(),
            user_key: Vec::new(),
            version: 2,
            revision: 3,
            key_bytes: 16,
            owner: decode_hex("1d1ff7011663408661b4e24dcc3db7f376f0fb37e02315869a2bc769442d356a"),
            user: decode_hex("b1b69d3ca429a6fa7a6f27d64de7cd5391f1dd59d88e5b6e5e0a7954217a8f78"),
            permissions: -3104,
            file_id: decode_hex("66d36a30a97e0f16f39955c6221e0c2a"),
            encrypt_metadata: true,
            stream_method: CipherMethod::Rc4,
            string_method: CipherMethod::Rc4,
        }
    }

    fn r2_parameters() -> Parameters {
        Parameters {
            owner_key: Vec::new(),
            user_key: Vec::new(),
            version: 1,
            revision: 2,
            key_bytes: 5,
            owner: decode_hex("b6a1b9d560569b24420c051b2a98b59e988290b5c95d9f671e8f0b53e123a980"),
            user: decode_hex("92c13d6657a15b2e15fd84026e7acc49e68f1241f55328c3c5289f3f7760f51f"),
            permissions: -64,
            file_id: decode_hex("66d36a30a97e0f16f39955c6221e0c2a"),
            encrypt_metadata: true,
            stream_method: CipherMethod::Rc4,
            string_method: CipherMethod::Rc4,
        }
    }

    fn encrypted_fixture(encryption_dictionary: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-1.6\n".to_vec();
        let encryption = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n");
        bytes.extend_from_slice(encryption_dictionary);
        bytes.extend_from_slice(b"\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{encryption:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(
            b"trailer\n<< /Size 2 /Encrypt 1 0 R /ID [<66d36a30a97e0f16f39955c6221e0c2a><31415926535897932384626433832795>] >>\nstartxref\n",
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(81), Arc::<[u8]>::from(bytes))
    }

    #[test]
    fn password_padding_matches_the_pdf_reference() {
        assert_eq!(pad_password(b""), super::PASSWORD_PADDING);
        assert_eq!(&pad_password(b"abc")[..6], b"abc\x28\xbf\x4e");
        assert_eq!(pad_password(&[b'x'; 40]), [b'x'; 32]);
    }

    #[test]
    fn authenticates_qpdf_r4_owner_and_empty_user_passwords() {
        let parameters = r4_parameters();
        let user = authenticate_user_password(&parameters, b"")
            .expect("valid computation")
            .expect("qpdf empty user password");
        assert_eq!(user, decode_hex("474258b004f2ce0017adeb6e79574357"));

        let owner = authenticate(&parameters, b"").expect("qpdf owner password");
        assert_eq!(owner.access_level(), AccessLevel::Owner);
        assert_eq!(owner.stream_method(), CipherMethod::Aes128);
        assert_eq!(
            authenticate(&r4_parameters(), b"wrong")
                .expect_err("incorrect password")
                .kind(),
            SecurityErrorKind::InvalidPassword
        );
    }

    fn r6_fixture() -> ByteStore {
        encrypted_fixture(
            b"<< /CF << /StdCF << /AuthEvent /DocOpen /CFM /AESV3 /Length 32 >> >> \
/Filter /Standard /Length 256 /V 5 /R 6 /P -4 /StmF /StdCF /StrF /StdCF \
/O <4bbc0c054d91cc7495ff53f0f6527eca98a66e3b5037f715d0a188ef3e0d48f44f56414c53414c544f4b455953414c54> \
/U <fe4b4e3bc70c96beeb9e5b24b45a082389fcc86c87abd228882177f2a8386b415556414c53414c54554b455953414c54> \
/OE <fc12d1c0f8ecfec015ce55a5db3baadea00f53ce87514e11e49a3aecc628e890> \
/UE <049c3dd3f275334210913d6ffaf7cba6140d0b39fbf20ac326821a705dff259f> >>",
        )
    }

    fn r6_security(password: &[u8]) -> Result<AuthenticatedSecurity, SecurityError> {
        let source = r6_fixture();
        let chain =
            parse_revision_chain_strict(&source, XrefLimits::default()).expect("valid fixture");
        let index = RevisionIndex::from_chain(&chain).expect("unambiguous fixture");
        authenticate_standard_password(&source, &chain, &index, password, ResolveLimits::default())
    }

    #[test]
    fn recovers_the_file_key_of_a_two_point_zero_document_from_either_password() {
        const KEY: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";

        let user = r6_security(b"").expect("the empty user password");
        assert_eq!(user.access_level(), AccessLevel::User);
        assert_eq!(user.stream_method(), CipherMethod::Aes256);
        assert_eq!(user.file_key.to_vec(), decode_hex(KEY));

        let owner = r6_security(b"owner").expect("the owner password");
        assert_eq!(owner.access_level(), AccessLevel::Owner);
        assert_eq!(owner.file_key.to_vec(), decode_hex(KEY));

        assert_eq!(
            r6_security(b"neither")
                .expect_err("a password that is neither")
                .kind(),
            SecurityErrorKind::InvalidPassword
        );
    }

    #[test]
    fn a_two_point_zero_string_decrypts_with_the_recovered_key() {
        let security = r6_security(b"").expect("the empty user password");
        let ciphertext =
            decode_hex("000102030405060708090a0b0c0d0e0f172317e18fef0754867224ba8a8030fd");
        let plain = security
            .decrypt_string(Reference::new(1, 0), &ciphertext)
            .expect("the string decrypts");
        assert_eq!(plain, b"panpdf");
    }

    #[test]
    fn a_document_that_encrypts_nothing_opens_without_a_password() {
        let source = encrypted_fixture(
            b"<< /CF << /StdCF << /AuthEvent /EFOpen /CFM /AESV3 /Length 32 >> >> \
/Filter /Standard /Length 256 /V 5 /R 6 /P -4 /StmF /Identity /StrF /Identity \
/O <4bbc0c054d91cc7495ff53f0f6527eca98a66e3b5037f715d0a188ef3e0d48f44f56414c53414c544f4b455953414c54> \
/U <fe4b4e3bc70c96beeb9e5b24b45a082389fcc86c87abd228882177f2a8386b415556414c53414c54554b455953414c54> \
/OE <fc12d1c0f8ecfec015ce55a5db3baadea00f53ce87514e11e49a3aecc628e890> \
/UE <049c3dd3f275334210913d6ffaf7cba6140d0b39fbf20ac326821a705dff259f> >>",
        );
        let chain =
            parse_revision_chain_strict(&source, XrefLimits::default()).expect("valid fixture");
        let index = RevisionIndex::from_chain(&chain).expect("unambiguous fixture");
        let security = authenticate_standard_password(
            &source,
            &chain,
            &index,
            b"not the password",
            ResolveLimits::default(),
        )
        .expect("nothing is encrypted, so nothing needed unlocking");
        assert_eq!(security.access_level(), AccessLevel::Unauthenticated);
        assert!(security.file_key.is_empty());
    }

    #[test]
    fn permissions_are_read_from_either_spelling_of_the_same_bits() {
        assert_eq!(permission_bits(-4).expect("signed"), -4);
        assert_eq!(permission_bits(4_294_967_292).expect("unsigned"), -4);
        assert_eq!(
            permission_bits(4_294_967_296)
                .expect_err("wider than the field")
                .kind(),
            SecurityErrorKind::InvalidEntry("P")
        );
    }

    #[test]
    fn opens_a_complete_qpdf_r4_dictionary_through_the_revision_index() {
        let source = encrypted_fixture(
            b"<< /CF << /StdCF << /AuthEvent /DocOpen /CFM /AESV2 /Length 16 >> >> /Filter /Standard /Length 128 /O <36451bd39d753b7c1d10922c28e6665aa4f3353fb0348b536893e3b1db5c579b> /P -4 /R 4 /StmF /StdCF /StrF /StdCF /U <32c27288b9ec6a4fab94e6188828595c0122456a91bae5134273a6db134c87c4> /V 4 >>",
        );
        let chain = parse_revision_chain_strict(&source, XrefLimits::default())
            .expect("valid encrypted fixture");
        let index = RevisionIndex::from_chain(&chain).expect("unambiguous fixture");
        let security =
            authenticate_standard_password(&source, &chain, &index, b"", ResolveLimits::default())
                .expect("empty qpdf password");
        assert_eq!(security.access_level(), AccessLevel::Owner);
        assert_eq!(security.stream_method(), CipherMethod::Aes128);
        assert_eq!(security.string_method(), CipherMethod::Aes128);
    }

    #[test]
    fn rejects_unsupported_revisions_and_invalid_permission_bits_before_password_work() {
        for (dictionary, expected) in [
            (
                &b"<< /Filter /Standard /Length 256 /O <000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000> /P -4 /R 6 /U <000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000> /V 5 >>"[..],
                SecurityErrorKind::MissingEntry("OE"),
            ),
            (
                &b"<< /Filter /Standard /Length 256 /O <00> /P -4 /R 6 /U <00> /V 5 >>"[..],
                SecurityErrorKind::InvalidOwnerUserLength,
            ),
            (
                &b"<< /Filter /Standard /Length 256 /O <000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000> /P -4 /R 7 /U <000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000> /V 5 >>"[..],
                SecurityErrorKind::UnsupportedRevision {
                    version: 5,
                    revision: 7,
                },
            ),
            (
                &b"<< /Filter /Standard /Length 40 /O <b6a1b9d560569b24420c051b2a98b59e988290b5c95d9f671e8f0b53e123a980> /P 0 /R 2 /U <92c13d6657a15b2e15fd84026e7acc49e68f1241f55328c3c5289f3f7760f51f> /V 1 >>"[..],
                SecurityErrorKind::InvalidEntry("P"),
            ),
        ] {
            let source = encrypted_fixture(dictionary);
            let chain = parse_revision_chain_strict(&source, XrefLimits::default())
                .expect("valid fixture syntax");
            let index = RevisionIndex::from_chain(&chain).expect("unambiguous fixture");
            assert_eq!(
                authenticate_standard_password(
                    &source,
                    &chain,
                    &index,
                    b"anything",
                    ResolveLimits::default(),
                )
                .expect_err("unsupported or invalid protection parameters")
                .kind(),
                expected
            );
        }
    }

    #[test]
    fn distinguishes_qpdf_r3_user_and_owner_passwords_and_accepts_r2() {
        let user = authenticate(&r3_parameters(), b"view").expect("qpdf R3 user password");
        assert_eq!(user.access_level(), AccessLevel::User);
        assert!(!user.may_modify_content());
        let owner = authenticate(&r3_parameters(), b"master").expect("qpdf R3 owner password");
        assert_eq!(owner.access_level(), AccessLevel::Owner);

        let shared = authenticate(&r2_parameters(), b"view").expect("qpdf R2 shared password");
        assert!(owner.may_modify_content());
        assert!(
            authenticate(&r4_parameters(), b"")
                .unwrap()
                .may_modify_content()
        );
        assert_eq!(shared.access_level(), AccessLevel::Owner);
    }

    #[test]
    fn printing_is_allowed_as_bits_3_and_12_say() {
        use super::PrintAllowance::{self, Degraded, Faithful, Refused};
        assert_eq!(PrintAllowance::of(3, -4), Faithful, "every bit set");
        assert_eq!(
            PrintAllowance::of(3, !2048),
            Degraded,
            "print, not faithfully"
        );
        assert_eq!(
            PrintAllowance::of(2, !2048),
            Faithful,
            "bit 12 is from revision 3"
        );
        assert_eq!(PrintAllowance::of(4, !4), Refused, "no print bit");
        assert_eq!(PrintAllowance::of(2, -64), Refused);
        let user = authenticate(&r3_parameters(), b"view").expect("qpdf R3 user password");
        assert_eq!(
            user.print_allowance(),
            Refused,
            "qpdf's -3104 withholds printing"
        );
        let owner = authenticate(&r3_parameters(), b"master").expect("qpdf R3 owner password");
        assert_eq!(owner.print_allowance(), Faithful);
        let open = authenticate(&r4_parameters(), b"").expect("R4 with no password");
        assert_eq!(open.print_allowance(), Faithful);
    }

    #[test]
    fn rc4_matches_the_published_known_answer() {
        assert_eq!(
            rc4_process(b"Plaintext", b"Key"),
            Ok(decode_hex("bbf316e8d940af0ad3"))
        );
    }

    #[test]
    fn derives_the_pdf_object_key_with_little_endian_object_identity() {
        let key = object_key(
            &decode_hex("474258b004f2ce0017adeb6e79574357"),
            Reference::new(79, 0),
            true,
        );
        assert_eq!(&*key, &decode_hex("3226e81e48c58a559448a8ed995a8ba0"));
    }

    #[test]
    fn a_stream_written_into_an_aes_document_decrypts_back_behind_a_fresh_vector() {
        let plaintext = b"written after an edit";
        let reference = Reference::new(5, 0);
        let aes128 = authenticate(&r4_parameters(), b"").expect("qpdf password");
        let aes256 = r6_security(b"").expect("the empty user password");
        for (name, security) in [("AES-128", &aes128), ("AES-256", &aes256)] {
            let first = security
                .encrypt_stream(reference, plaintext)
                .expect("AES is written");
            assert_eq!(first.len(), 16 + 32, "{name}");
            assert_eq!(
                security.decrypt_stream(reference, &first),
                Ok(plaintext.to_vec()),
                "{name}"
            );
            let second = security
                .encrypt_stream(reference, plaintext)
                .expect("AES is written");
            assert_ne!(
                first[..16],
                second[..16],
                "{name}: a fresh vector each write"
            );
            assert_eq!(
                security.decrypt_stream(reference, &second),
                Ok(plaintext.to_vec()),
                "{name}"
            );
        }
    }

    #[test]
    fn decrypts_a_qpdf_aes_string_and_rejects_bad_padding() {
        let security = authenticate(&r4_parameters(), b"").expect("qpdf password");
        let encrypted = decode_hex(
            "0e1c2a38465462707e8c9aa8b6c4d2e0e9899cdac1e580de96cee46c62bbe494cccd4890015eb51e9ebe9e163bd4e9db",
        );
        assert_eq!(
            security.decrypt_string(Reference::new(2, 0), &encrypted),
            Ok(b"D:20031010180432-03'00'".to_vec())
        );

        let mut malformed = encrypted;
        *malformed.last_mut().expect("ciphertext") ^= 1;
        assert_eq!(
            security
                .decrypt_string(Reference::new(2, 0), &malformed)
                .expect_err("bad PKCS#7 padding")
                .kind(),
            SecurityErrorKind::MalformedCiphertext
        );
    }

    #[test]
    fn decrypts_a_stream_before_applying_flate() {
        let security = authenticate(&r4_parameters(), b"").expect("qpdf password");
        let reference = Reference::new(5, 0);
        let key = object_key(&security.file_key, reference, true);
        let iv = [0x24_u8; 16];
        let flate = decode_hex("789c4b494d2eaa2c2851484a4dcb2f4a5570cb492c4905005166079b");
        let ciphertext = cbc::Encryptor::<aes::Aes128>::new_from_slices(&key, &iv)
            .expect("AES object key")
            .encrypt_padded_vec::<Pkcs7>(&flate);
        let mut encrypted = iv.to_vec();
        encrypted.extend_from_slice(&ciphertext);

        let mut bytes = format!(
            "5 0 obj\n<< /Length {} /Filter /FlateDecode >>\nstream\n",
            encrypted.len()
        )
        .into_bytes();
        bytes.extend_from_slice(&encrypted);
        bytes.extend_from_slice(b"\nendstream\nendobj\n");
        let source = ByteStore::new(SourceId::new(82), Arc::<[u8]>::from(bytes));
        let object = parse_indirect_object_strict(&source, 0, ParseLimits::default())
            .expect("encrypted stream envelope");
        let ObjectKind::Dictionary(dictionary) = object.value().kind() else {
            panic!("stream dictionary");
        };
        assert_eq!(
            security.decrypt_and_decode_stream(
                &source,
                reference,
                dictionary,
                object.stream().expect("stream"),
                20,
            ),
            Ok(b"decrypt before Flate".to_vec())
        );
    }

    #[test]
    fn with_no_operating_system_source_only_a_seed_gives_random_bytes() {
        std::thread::spawn(|| {
            let none: fn(&mut [u8]) -> bool = |_| false;
            let refused = super::random_bytes_from::<16>(none).unwrap_err();
            assert_eq!(refused.kind(), SecurityErrorKind::UnsupportedWriteCipher);
            assert_eq!(
                super::seed_random(&[0; 64]).unwrap_err().kind(),
                SecurityErrorKind::WeakRandomSeed
            );
            assert!(super::random_bytes_from::<16>(none).is_err());
            let seed: Vec<u8> = (0..64).map(|n: u8| n.wrapping_mul(37)).collect();
            super::seed_random(&seed).unwrap();
            let first = super::random_bytes_from::<16>(none).unwrap();
            let second = super::random_bytes_from::<16>(none).unwrap();
            assert_ne!(first, second);
            let os: fn(&mut [u8]) -> bool = |bytes| {
                bytes.fill(0xa5);
                true
            };
            assert_eq!(super::random_bytes_from::<4>(os).unwrap(), [0xa5; 4]);
        })
        .join()
        .unwrap();
    }
}
