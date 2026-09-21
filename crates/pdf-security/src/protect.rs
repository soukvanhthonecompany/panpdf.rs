use aes::cipher::{BlockCipherEncrypt, BlockModeEncrypt, KeyIvInit};
use zeroize::Zeroizing;

use super::{
    AuthenticatedSecurity, CipherMethod, PrintAllowance, SecurityError, SecurityErrorKind,
    random_bytes, two_point_zero_hash,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Table 22 is a row of independent flags, and this is that row"
)]
pub struct Allowed {
    pub print: PrintAllowance,
    pub modify: bool,
    pub copy: bool,
    pub annotate: bool,
    pub fill_forms: bool,
    pub assemble: bool,
}

impl Default for Allowed {
    fn default() -> Self {
        Self {
            print: PrintAllowance::Faithful,
            modify: true,
            copy: true,
            annotate: true,
            fill_forms: true,
            assemble: true,
        }
    }
}

impl Allowed {
    #[must_use]
    pub const fn flags(&self) -> i32 {
        let mut bits: u32 = 0xffff_f0c0;
        bits |= 1 << 9;
        if !matches!(self.print, PrintAllowance::Refused) {
            bits |= 1 << 2;
        }
        if matches!(self.print, PrintAllowance::Faithful) {
            bits |= 1 << 11;
        }
        if self.modify {
            bits |= 1 << 3;
        }
        if self.copy {
            bits |= 1 << 4;
        }
        if self.annotate {
            bits |= 1 << 5;
        }
        if self.fill_forms {
            bits |= 1 << 8;
        }
        if self.assemble {
            bits |= 1 << 10;
        }
        i32::from_ne_bytes(bits.to_ne_bytes())
    }
}

#[derive(Clone, Debug, Default)]
pub struct Wanted {
    pub user: Vec<u8>,
    pub owner: Vec<u8>,
    pub allowed: Allowed,
}

pub struct MadeProtection {
    pub dictionary: Vec<u8>,
    pub security: AuthenticatedSecurity,
}

const MOST_PASSWORD_BYTES: usize = 127;

pub fn make_protection(wanted: &Wanted) -> Result<MadeProtection, SecurityError> {
    let user = shortened(&wanted.user);
    let owner = shortened(&wanted.owner);
    let permissions = wanted.allowed.flags();
    let file_key = Zeroizing::new(random_bytes::<32>()?.to_vec());

    let user_validation = random_bytes::<8>()?;
    let user_salt = random_bytes::<8>()?;
    let mut user_value = two_point_zero_hash(6, user, &user_validation, &[])?.to_vec();
    user_value.extend_from_slice(&user_validation);
    user_value.extend_from_slice(&user_salt);
    let user_wrapping = two_point_zero_hash(6, user, &user_salt, &[])?;
    let user_key = wrap_file_key(&file_key, &user_wrapping)?;

    let owner_validation = random_bytes::<8>()?;
    let owner_salt = random_bytes::<8>()?;
    let mut owner_value = two_point_zero_hash(6, owner, &owner_validation, &user_value)?.to_vec();
    owner_value.extend_from_slice(&owner_validation);
    owner_value.extend_from_slice(&owner_salt);
    let owner_wrapping = two_point_zero_hash(6, owner, &owner_salt, &user_value)?;
    let owner_key = wrap_file_key(&file_key, &owner_wrapping)?;

    let perms = permissions_block(permissions, &file_key)?;

    let dictionary = format!(
        "<< /Filter /Standard /V 5 /R 6 /Length 256 /P {permissions} \
         /CF << /StdCF << /CFM /AESV3 /AuthEvent /DocOpen /Length 32 >> >> \
         /StmF /StdCF /StrF /StdCF /EncryptMetadata true \
         /U <{}> /UE <{}> /O <{}> /OE <{}> /Perms <{}> >>",
        hex(&user_value),
        hex(&user_key),
        hex(&owner_value),
        hex(&owner_key),
        hex(&perms),
    )
    .into_bytes();

    Ok(MadeProtection {
        dictionary,
        security: AuthenticatedSecurity {
            version: 5,
            revision: 6,
            access: super::AccessLevel::Owner,
            permissions,
            encrypt_metadata: true,
            stream_method: CipherMethod::Aes256,
            string_method: CipherMethod::Aes256,
            file_key,
        },
    })
}

fn shortened(password: &[u8]) -> &[u8] {
    &password[..password.len().min(MOST_PASSWORD_BYTES)]
}

fn wrap_file_key(file_key: &[u8], wrapping: &[u8]) -> Result<Vec<u8>, SecurityError> {
    if file_key.len() != 32 {
        return Err(SecurityError::new(SecurityErrorKind::InvalidCipherKey));
    }
    let mut blocks = [
        aes::cipher::Array::from([0_u8; 16]),
        aes::cipher::Array::from([0_u8; 16]),
    ];
    blocks[0].copy_from_slice(&file_key[..16]);
    blocks[1].copy_from_slice(&file_key[16..]);
    cbc::Encryptor::<aes::Aes256>::new_from_slices(wrapping, &[0_u8; 16])
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?
        .encrypt_blocks(&mut blocks);
    let mut out = Vec::with_capacity(32);
    out.extend_from_slice(&blocks[0]);
    out.extend_from_slice(&blocks[1]);
    Ok(out)
}

fn permissions_block(permissions: i32, file_key: &[u8]) -> Result<[u8; 16], SecurityError> {
    let mut block = [0_u8; 16];
    block[..4].copy_from_slice(&permissions.to_le_bytes());
    block[4..8].copy_from_slice(&[0xff; 4]);
    block[8] = b'T';
    block[9..12].copy_from_slice(b"adb");
    block[12..].copy_from_slice(&random_bytes::<4>()?);

    let cipher = <aes::Aes256 as aes::cipher::KeyInit>::new_from_slice(file_key)
        .map_err(|_| SecurityError::new(SecurityErrorKind::InvalidCipherKey))?;
    let mut one = aes::cipher::Array::from(block);
    cipher.encrypt_block(&mut one);
    Ok(one.into())
}

pub fn random_identifier() -> Result<String, SecurityError> {
    Ok(hex(&random_bytes::<16>()?))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use pdf_bytes::{ByteStore, SourceId};
    use pdf_syntax::{
        Reference, ResolveLimits, RevisionIndex, XrefLimits, parse_revision_chain_strict,
    };

    use super::{Allowed, Wanted, make_protection};
    use crate::{
        AccessLevel, CipherMethod, PrintAllowance, SecurityErrorKind,
        authenticate_standard_password,
    };

    fn document(dictionary: &[u8]) -> ByteStore {
        let mut bytes = b"%PDF-2.0\n".to_vec();
        let at = bytes.len();
        bytes.extend_from_slice(b"1 0 obj\n");
        bytes.extend_from_slice(dictionary);
        bytes.extend_from_slice(b"\nendobj\n");
        let xref = bytes.len();
        bytes.extend_from_slice(b"xref\n0 2\n0000000000 65535 f \n");
        bytes.extend_from_slice(format!("{at:010} 00000 n \n").as_bytes());
        bytes.extend_from_slice(
            b"trailer\n<< /Size 2 /Encrypt 1 0 R \
              /ID [<66d36a30a97e0f16f39955c6221e0c2a><31415926535897932384626433832795>] >>\n\
              startxref\n",
        );
        bytes.extend_from_slice(xref.to_string().as_bytes());
        bytes.extend_from_slice(b"\n%%EOF\n");
        ByteStore::new(SourceId::new(82), Arc::<[u8]>::from(bytes))
    }

    fn opened(
        dictionary: &[u8],
        password: &[u8],
    ) -> Result<crate::AuthenticatedSecurity, crate::SecurityError> {
        let source = document(dictionary);
        let chain = parse_revision_chain_strict(&source, XrefLimits::default()).expect("it parses");
        let index = RevisionIndex::from_chain(&chain).expect("one revision");
        authenticate_standard_password(&source, &chain, &index, password, ResolveLimits::default())
    }

    #[test]
    fn protection_made_here_is_protection_this_crate_authenticates() {
        let made = make_protection(&Wanted {
            user: b"\xe0\xb9\x80\xe0\xb8\x9b\xe0\xb8\xb4\xe0\xb8\x94".to_vec(),
            owner: b"master".to_vec(),
            allowed: Allowed::default(),
        })
        .expect("the protection is made");

        let user = opened(
            &made.dictionary,
            b"\xe0\xb9\x80\xe0\xb8\x9b\xe0\xb8\xb4\xe0\xb8\x94",
        )
        .expect("the user password opens it");
        assert_eq!(user.access_level(), AccessLevel::User);
        assert_eq!(user.revision(), 6);
        assert_eq!(user.version(), 5);
        assert_eq!(user.stream_method(), CipherMethod::Aes256);
        assert_eq!(user.string_method(), CipherMethod::Aes256);

        let owner = opened(&made.dictionary, b"master").expect("the owner password opens it");
        assert_eq!(owner.access_level(), AccessLevel::Owner);

        assert_eq!(user.file_key.to_vec(), owner.file_key.to_vec());
        assert_eq!(
            user.file_key.to_vec(),
            made.security.file_key.to_vec(),
            "and it is the key the document was encrypted with"
        );

        assert_eq!(
            opened(&made.dictionary, b"neither")
                .expect_err("a password that is neither")
                .kind(),
            SecurityErrorKind::InvalidPassword
        );
        assert_eq!(
            opened(&made.dictionary, b"")
                .expect_err("the empty password, when a user password was set")
                .kind(),
            SecurityErrorKind::InvalidPassword
        );
    }

    #[test]
    fn a_document_with_no_user_password_opens_for_anyone() {
        let made = make_protection(&Wanted {
            user: Vec::new(),
            owner: b"master".to_vec(),
            allowed: Allowed {
                print: PrintAllowance::Degraded,
                modify: false,
                copy: false,
                annotate: false,
                fill_forms: true,
                assemble: false,
            },
        })
        .expect("the protection is made");

        let reader = opened(&made.dictionary, b"").expect("anyone may open it");
        assert_eq!(reader.access_level(), AccessLevel::User);
        assert!(!reader.may_modify_content(), "which is what was asked for");
        assert_eq!(reader.print_allowance(), PrintAllowance::Degraded);

        let owner = opened(&made.dictionary, b"master").expect("the owner password");
        assert!(
            owner.may_modify_content(),
            "an owner is allowed everything whatever the flags say"
        );
        assert_eq!(owner.print_allowance(), PrintAllowance::Faithful);
    }

    #[test]
    fn what_a_document_is_told_to_allow_is_what_it_allows() {
        for (allowed, expected) in [
            (Allowed::default(), (true, true, true, true, true)),
            (
                Allowed {
                    print: PrintAllowance::Refused,
                    modify: false,
                    copy: true,
                    annotate: false,
                    fill_forms: false,
                    assemble: false,
                },
                (false, true, false, false, false),
            ),
        ] {
            let made = make_protection(&Wanted {
                user: Vec::new(),
                owner: b"o".to_vec(),
                allowed,
            })
            .expect("the protection is made");
            let read = opened(&made.dictionary, b"").expect("anyone may open it");
            let bit = |n: u32| read.permissions() & (1 << (n - 1)) != 0;
            assert_eq!(
                (bit(4), bit(5), bit(6), bit(9), bit(11)),
                expected,
                "{allowed:?}"
            );
            assert_eq!(read.print_allowance(), allowed.print);
            assert!(
                bit(10),
                "extracting for accessibility is no longer withheld"
            );
            assert!(
                read.permissions().trailing_zeros() >= 2,
                "bits 1 and 2 are reserved and shall be zero"
            );
        }
    }

    #[test]
    fn a_string_written_under_the_new_key_reads_back_through_it() {
        let made = make_protection(&Wanted {
            user: b"view".to_vec(),
            owner: b"master".to_vec(),
            allowed: Allowed::default(),
        })
        .expect("the protection is made");
        let at = Reference::new(7, 0);
        let plain = "รายงานประจำปี".as_bytes();
        let ciphertext = made
            .security
            .encrypt_string(at, plain)
            .expect("it encrypts");
        assert_ne!(ciphertext, plain, "it really is encrypted");

        let read = opened(&made.dictionary, b"view").expect("the user password");
        assert_eq!(
            read.decrypt_string(at, &ciphertext).expect("it decrypts"),
            plain
        );
    }

    #[test]
    fn the_permissions_block_says_what_the_flags_say() {
        use aes::cipher::BlockCipherDecrypt;

        let allowed = Allowed {
            print: PrintAllowance::Refused,
            modify: false,
            copy: true,
            annotate: false,
            fill_forms: true,
            assemble: false,
        };
        let made = make_protection(&Wanted {
            user: Vec::new(),
            owner: b"o".to_vec(),
            allowed,
        })
        .expect("the protection is made");

        let hex = String::from_utf8(made.dictionary.clone()).expect("ASCII");
        let at = hex.find("/Perms <").expect("it is written") + "/Perms <".len();
        let digits = &hex[at..at + 32];
        let mut block = [0_u8; 16];
        for (byte, pair) in block.iter_mut().zip(digits.as_bytes().chunks_exact(2)) {
            *byte = u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII"), 16)
                .expect("hexadecimal");
        }
        let mut one = aes::cipher::Array::from(block);
        <aes::Aes256 as aes::cipher::KeyInit>::new_from_slice(&made.security.file_key)
            .expect("the file key")
            .decrypt_block(&mut one);

        assert_eq!(
            i32::from_le_bytes(one[..4].try_into().expect("four bytes")),
            allowed.flags(),
            "the permissions, low-order byte first"
        );
        assert_eq!(&one[4..8], &[0xff; 4]);
        assert_eq!(one[8], b'T', "/EncryptMetadata, which this writes as true");
        assert_eq!(&one[9..12], b"adb");
    }

    #[test]
    fn the_key_is_new_every_time() {
        let wanted = Wanted {
            user: b"same".to_vec(),
            ..Wanted::default()
        };
        let once = make_protection(&wanted).expect("made");
        let twice = make_protection(&wanted).expect("made");
        assert_ne!(
            once.security.file_key.to_vec(),
            twice.security.file_key.to_vec()
        );
        assert_ne!(once.dictionary, twice.dictionary);
    }
}
