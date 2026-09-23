use super::{SALT, hex, hmac, lock, looks_like_ours, sha256, unlock};

#[test]
fn sha256_gives_the_published_answers() {
    assert_eq!(
        hex(&sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(&sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(&sha256(
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
        )),
        "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
    );
    assert_eq!(
        hex(&sha256(&[b'a'; 64])),
        "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb"
    );
}

#[test]
fn hmac_gives_the_published_answers() {
    assert_eq!(
        hex(&hmac(&[0x0b; 20], b"Hi There")),
        "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
    );
    assert_eq!(
        hex(&hmac(b"Jefe", b"what do ya want for nothing?")),
        "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
    );
    assert_eq!(
        hex(&hmac(
            &[0xaa; 131],
            b"Test Using Larger Than Block-Size Key - Hash Key First"
        )),
        "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54"
    );
}

fn machine() -> Vec<u8> {
    b"this machine's own secret".to_vec()
}

fn salt() -> Vec<u8> {
    (0..SALT).map(|at| u8::try_from(at).unwrap_or(0)).collect()
}

#[test]
fn a_key_locked_here_comes_back_here() {
    let secret = "gsk_thisisnotarealkeyatall000000000000";
    let line = lock(secret, &machine(), &salt()).expect("it locks");
    assert_eq!(unlock(&line, &machine()).as_deref(), Some(secret));
}

#[test]
fn the_file_holds_none_of_the_key() {
    let secret = "gsk_ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let line = lock(secret, &machine(), &salt()).expect("it locks");
    assert!(!line.contains(secret));
    assert!(!line.contains("ABCDEFGH"));
    assert!(!line.contains(&hex(secret.as_bytes())));
}

#[test]
fn a_line_from_another_machine_reads_as_nothing() {
    let line = lock("a secret", &machine(), &salt()).expect("it locks");
    assert_eq!(unlock(&line, b"some other machine"), None);
    assert!(looks_like_ours(&line));
}

#[test]
fn an_edited_line_is_refused() {
    let line = lock("a secret", &machine(), &salt()).expect("it locks");
    let fields: Vec<&str> = line.trim_end().split('\t').collect();
    assert_eq!(fields.len(), 4);
    for at in 1..4 {
        let mut edited = fields.clone();
        let mut letters: Vec<char> = edited[at].chars().collect();
        letters[0] = if letters[0] == 'a' { 'b' } else { 'a' };
        let changed: String = letters.into_iter().collect();
        edited[at] = &changed;
        assert_eq!(unlock(&edited.join("\t"), &machine()), None, "field {at}");
    }
}

#[test]
fn the_same_key_written_twice_looks_different() {
    let other: Vec<u8> = (0..SALT)
        .map(|at| u8::try_from(at + 9).unwrap_or(0))
        .collect();
    let one = lock("a secret", &machine(), &salt()).expect("it locks");
    let two = lock("a secret", &machine(), &other).expect("it locks");
    assert_ne!(one, two);
    assert_eq!(unlock(&one, &machine()).as_deref(), Some("a secret"));
    assert_eq!(unlock(&two, &machine()).as_deref(), Some("a secret"));
}

#[test]
fn what_cannot_be_locked_or_read_is_refused() {
    assert_eq!(lock("", &machine(), &salt()), None);
    assert_eq!(lock("a secret", &machine(), &[0; 4]), None);
    assert_eq!(unlock("", &machine()), None);
    assert_eq!(unlock("panpdf-key-9\tff\tff\tff", &machine()), None);
    assert_eq!(unlock("not ours at all", &machine()), None);
    assert!(!looks_like_ours("not ours at all"));
    assert_eq!(unlock("panpdf-key-1\tff\tff\tff", &machine()), None);
    let line = lock("a secret", &machine(), &salt()).expect("it locks");
    let fields: Vec<&str> = line.trim_end().split('\t').collect();
    let short = format!("{}\t{}\t{}\t{}", fields[0], fields[1], "abc", fields[3]);
    assert_eq!(unlock(&short, &machine()), None);
}

#[test]
fn a_long_key_comes_back_whole() {
    let secret: String = std::iter::repeat_n("0123456789abcdef", 12).collect();
    assert_eq!(secret.len(), 192);
    let line = lock(&secret, &machine(), &salt()).expect("it locks");
    assert_eq!(unlock(&line, &machine()).as_deref(), Some(secret.as_str()));
}
