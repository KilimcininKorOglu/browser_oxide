//! BoringSSL TLS configuration with Chrome 147 fingerprint.
//!
//! Configures TLS to produce a ClientHello identical to Chrome 147,
//! including cipher suites, curves, signature algorithms, extensions,
//! and certificate compression — all in the exact order that produces
//! the correct JA3/JA4 fingerprint.

use crate::stealth::{DeviceClass, StealthProfile};
use btls::ssl::{
    CertificateCompressionAlgorithm, CertificateCompressor, ConnectConfiguration, ExtensionType,
    KeyShare, SslConnector, SslMethod, SslOptions, SslVersion,
};
use btls::x509::store::X509StoreBuilder;
use btls::x509::X509;
use foreign_types::ForeignTypeRef;
use tokio::net::TcpStream;
use tokio_btls::SslStream;

use crate::net::error::NetError;

/// The Chrome major version whose **verified-real** ClientHello / H2
/// fingerprint these constants reproduce.
///
/// Bumped 147 → 153: live measurement against real Chrome 153
/// (tls.peet.ws JA3/JA4, same machine) showed Chrome 153 always sends the
/// `trust_anchors` extension (51764, empty anchor list when the server
/// advertises no `tls-trust-anchors` parameter), which the older BoringSSL
/// could not emit at all. With btls 0.5.6 the hello is 153-class:
/// 17 extensions + 2 GREASE, MLKEM768 key shares, current ALPS payload.
///
/// Two older claims this supersedes (kept here so nobody reverts):
/// 1. Chrome's ClientHello was treated as version-stable across majors —
///    trust_anchors proves the stack DOES rev between majors.
/// 2. JA4 still does not encode the Chrome version (counts + sorted
///    hashes only), so a JA4-vs-UA cross-check verifies the *family*.
///
/// This constant exists so the coherence is **machine-checked** (see the
/// `tls_fingerprint_vectors_no_silent_drift` test) and the rationale is
/// one `grep` away.
pub const TLS_CHROME_MAJOR: u32 = 153;

/// The Chrome major every desktop Chrome preset's `user_agent`
/// advertises. Equals [`TLS_CHROME_MAJOR`]: UA and TLS agree.
pub const UA_CHROME_MAJOR: u32 = 153;

/// Chrome 147 cipher suite list (order is critical for JA3 fingerprint).
const CIPHER_LIST: &str = concat!(
    "TLS_AES_128_GCM_SHA256",
    ":TLS_AES_256_GCM_SHA384",
    ":TLS_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    ":TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    ":TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    ":TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    ":TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
    ":TLS_RSA_WITH_AES_128_GCM_SHA256",
    ":TLS_RSA_WITH_AES_256_GCM_SHA384",
    ":TLS_RSA_WITH_AES_128_CBC_SHA",
    ":TLS_RSA_WITH_AES_256_CBC_SHA",
);

/// Chrome 153 signature algorithms, in Chrome's order: the three
/// post-quantum ones first, then the usual eight. Measured live against real
/// Chrome 153 on this machine — the ClientHello carried
/// `0x904, 0x905, 0x906` ahead of `ecdsa_secp256r1_sha256`.
///
/// The `mldsa*` names are the ones the vendored BoringSSL knows
/// (`vendor/btls-sys/PATCHES.md`); stock btls-sys refuses them and the
/// extension is then short by three codepoints, which is what the third part
/// of JA4 hashes.
const SIGALGS_LIST: &str = concat!(
    "mldsa44:mldsa65:mldsa87",
    ":ecdsa_secp256r1_sha256",
    ":rsa_pss_rsae_sha256",
    ":rsa_pkcs1_sha256",
    ":ecdsa_secp384r1_sha384",
    ":rsa_pss_rsae_sha384",
    ":rsa_pkcs1_sha384",
    ":rsa_pss_rsae_sha512",
    ":rsa_pkcs1_sha512",
);

/// Brotli certificate compressor (IANA algo 2) for the
/// `compress_certificate` extension. btls 0.5.6 takes user
/// implementations via the `CertificateCompressor` trait instead of
/// shipping built-ins.
struct BrotliCertCompressor;

impl CertificateCompressor for BrotliCertCompressor {
    const ALGORITHM: CertificateCompressionAlgorithm = CertificateCompressionAlgorithm::BROTLI;
    const CAN_COMPRESS: bool = true;
    const CAN_DECOMPRESS: bool = true;

    fn compress<W>(&self, input: &[u8], output: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        use std::io::Write as _;
        let mut writer = brotli::CompressorWriter::new(output, 4096, 11, 22);
        writer.write_all(input)?;
        writer.flush()
    }

    fn decompress<W>(&self, input: &[u8], output: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        let mut reader = brotli::Decompressor::new(input, 4096);
        std::io::copy(&mut reader, output)?;
        Ok(())
    }
}

/// Zlib certificate compressor (IANA algo 1): Safari and Firefox arms.
struct ZlibCertCompressor;

impl CertificateCompressor for ZlibCertCompressor {
    const ALGORITHM: CertificateCompressionAlgorithm = CertificateCompressionAlgorithm::ZLIB;
    const CAN_COMPRESS: bool = true;
    const CAN_DECOMPRESS: bool = true;

    fn compress<W>(&self, input: &[u8], output: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        use std::io::Write as _;
        let mut encoder = flate2::write::ZlibEncoder::new(output, flate2::Compression::default());
        encoder.write_all(input)?;
        encoder.finish()?;
        Ok(())
    }

    fn decompress<W>(&self, input: &[u8], output: &mut W) -> std::io::Result<()>
    where
        W: std::io::Write,
    {
        let mut decoder = flate2::read::ZlibDecoder::new(input);
        std::io::copy(&mut decoder, output)?;
        Ok(())
    }
}

/// Chrome desktop elliptic curves (Chrome 131+ uses MLKEM768).
/// String form for `SSL_CTX_set1_curves_list`.
const CURVES_DESKTOP: &str = "X25519MLKEM768:X25519:P-256:P-384";

/// Chrome Android elliptic curves. Kyber768Draft00 (deprecated) was the
/// canonical Chrome 124-130 PQ curve; Chrome 131+ desktop replaced it with
/// MLKEM768 (codepoint 4588). A reference Chrome 131 Android capture
/// shows no PQ at all (just 29/23/24), but Chrome Android shares the
/// desktop codebase and by Chrome 147+ should have rolled MLKEM — verify
/// against a fresh Pixel capture if regressions appear.
const CURVES_ANDROID: &str = CURVES_DESKTOP;

/// iOS Safari 18 cipher suite list (20 ciphers, Apple's order). Per a
/// reference Safari iOS 18 TLS capture.
/// Distinct from Chrome desktop (15 ciphers): includes 3DES_EDE_CBC_SHA at
/// the tail and an extra RSA_WITH_3DES_EDE_CBC_SHA. Cipher order matters
/// for JA3.
const CIPHER_LIST_SAFARI_IOS: &str = concat!(
    "TLS_AES_128_GCM_SHA256",
    ":TLS_AES_256_GCM_SHA384",
    ":TLS_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    ":TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    ":TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    ":TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA",
    ":TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA",
    ":TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
    ":TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    ":TLS_RSA_WITH_AES_256_GCM_SHA384",
    ":TLS_RSA_WITH_AES_128_GCM_SHA256",
    ":TLS_RSA_WITH_AES_256_CBC_SHA",
    ":TLS_RSA_WITH_AES_128_CBC_SHA",
    ":TLS_ECDHE_ECDSA_WITH_3DES_EDE_CBC_SHA",
    ":TLS_ECDHE_RSA_WITH_3DES_EDE_CBC_SHA",
    ":TLS_RSA_WITH_3DES_EDE_CBC_SHA",
);

/// iOS Safari signature algorithms (10 entries, includes the duplicated
/// `rsa_pss_rsae_sha384` Apple quirk we must reproduce verbatim).
/// Reference Safari TLS captures include the duplicate.
const SIGALGS_LIST_SAFARI_IOS: &str = concat!(
    "ecdsa_secp256r1_sha256",
    ":rsa_pss_rsae_sha256",
    ":rsa_pkcs1_sha256",
    ":ecdsa_secp384r1_sha384",
    ":rsa_pss_rsae_sha384",
    ":rsa_pss_rsae_sha384",
    ":rsa_pkcs1_sha384",
    ":rsa_pss_rsae_sha512",
    ":rsa_pkcs1_sha512",
    ":rsa_pkcs1_sha1",
);

/// iOS Safari 18 elliptic curves. No PQ (MLKEM lands in iOS 26 per Apple's
/// PQC support page). Adds P-521 vs Chrome desktop.
const CURVES_SAFARI_IOS: &str = "X25519:P-256:P-384:P-521";

/// iOS Safari 18 extension order, as TLS wire IDs. Real Safari emits a
/// FIXED order (no shuffle): server_name, extended_master_secret,
/// renegotiate, supported_groups, ec_point_formats, ALPN, status_request,
/// signature_algorithms, signed_certificate_timestamp, key_share,
/// psk_key_exchange_modes, supported_versions, cert_compression.
/// (GREASE and PADDING are auto-emitted by BoringSSL outside the order
/// table; PADDING positional ordering requires raw extension injection —
/// deferred.)
const SAFARI_IOS_EXTENSION_ORDER: &[u16] = &[
    0,     // server_name
    23,    // extended_master_secret
    65281, // renegotiate
    10,    // supported_groups
    11,    // ec_point_formats
    16,    // application_layer_protocol_negotiation (ALPN)
    5,     // status_request
    13,    // signature_algorithms
    18,    // certificate_timestamp
    51,    // key_share
    45,    // psk_key_exchange_modes
    43,    // supported_versions
    27,    // cert_compression (compress_certificate)
];

/// Firefox 135 (NSS) cipher suite list — 17 ciphers, NSS order. Distinct
/// from Chrome's 15: NSS leads TLS1.3 with AES-128-GCM, CHACHA20, AES-256-GCM
/// (CHACHA before AES-256), then the ECDHE-ECDSA/RSA GCM pairs, then the CBC
/// block (ECDSA before RSA, 256 before 128 in NSS's CBC ordering), then the
/// two RSA-GCM and two RSA-CBC fallbacks. Yields the Firefox JA4 cipher hash
/// `5b57614c22b0` (vs Chrome's). Per reference Firefox TLS captures.
const CIPHER_LIST_FIREFOX: &str = concat!(
    "TLS_AES_128_GCM_SHA256",
    ":TLS_CHACHA20_POLY1305_SHA256",
    ":TLS_AES_256_GCM_SHA384",
    ":TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256",
    ":TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256",
    ":TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384",
    ":TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384",
    ":TLS_ECDHE_ECDSA_WITH_AES_256_CBC_SHA",
    ":TLS_ECDHE_ECDSA_WITH_AES_128_CBC_SHA",
    ":TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA",
    ":TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA",
    ":TLS_RSA_WITH_AES_128_GCM_SHA256",
    ":TLS_RSA_WITH_AES_256_GCM_SHA384",
    ":TLS_RSA_WITH_AES_128_CBC_SHA",
    ":TLS_RSA_WITH_AES_256_CBC_SHA",
);

/// Firefox 135 (NSS) signature algorithms — 11 entries, NSS order: the three
/// ECDSA curves first, then RSA-PSS, then RSA-PKCS1, then the SHA-1 tail
/// (ecdsa_sha1, rsa_pkcs1_sha1). Yields the Firefox JA4 sigalg hash
/// `3d5424432f57`.
const SIGALGS_LIST_FIREFOX: &str = concat!(
    "ecdsa_secp256r1_sha256",
    ":ecdsa_secp384r1_sha384",
    ":ecdsa_secp521r1_sha512",
    ":rsa_pss_rsae_sha256",
    ":rsa_pss_rsae_sha384",
    ":rsa_pss_rsae_sha512",
    ":rsa_pkcs1_sha256",
    ":rsa_pkcs1_sha384",
    ":rsa_pkcs1_sha512",
    ":ecdsa_sha1",
    ":rsa_pkcs1_sha1",
);

/// Firefox 135 supported_groups. NSS appends the two FFDHE groups
/// (ffdhe2048, ffdhe3072) after the EC curves — a hard Firefox signature no
/// Chrome build sends. X25519MLKEM768 leads (Firefox shipped PQ key-share by
/// default in 132+). P-521 present (Chrome desktop omits it).
const CURVES_FIREFOX: &str = "X25519MLKEM768:X25519:P-256:P-384:P-521:ffdhe2048:ffdhe3072";

/// Firefox 135 delegated_credentials (ext 0x22) sigalg list — Firefox-only.
/// The four ECDSA sigalgs NSS advertises in the delegated-credential ext.
const FIREFOX_DELEGATED_CREDENTIALS: &str = concat!(
    "ecdsa_secp256r1_sha256",
    ":ecdsa_secp384r1_sha384",
    ":ecdsa_secp521r1_sha512",
    ":ecdsa_sha1",
);

/// Firefox 135 record_size_limit (ext 0x1c) value: 0x4001 (16385).
const FIREFOX_RECORD_SIZE_LIMIT: u16 = 0x4001;

/// Firefox 135 extension order, as TLS wire IDs (FIXED order every
/// handshake — NSS does not shuffle). 15 extensions → the Firefox
/// `t13d1715h2` JA4 count.
/// Delegated_credentials (34) and record_size_limit (28) are hard
/// Firefox/NSS signatures absent from every Chrome build. Order verified
/// against a reference Firefox 135 TLS capture — iterate if the JA4
/// ext-hash diverges.
const FIREFOX_EXTENSION_ORDER: &[u16] = &[
    0,     // server_name
    23,    // extended_master_secret
    65281, // renegotiation_info
    10,    // supported_groups
    11,    // ec_point_formats
    35,    // session_ticket
    16,    // ALPN
    5,     // status_request
    34,    // delegated_credentials (0x22) — Firefox-only
    51,    // key_share
    43,    // supported_versions
    13,    // signature_algorithms
    45,    // psk_key_exchange_modes
    28,    // record_size_limit (0x1c) — Firefox-only
    65037, // encrypted_client_hello (ECH grease)
];

/// ALPN protocols: h2 + http/1.1
const ALPN_PROTOS: &[u8] = b"\x02h2\x08http/1.1";

use rand::prelude::SliceRandom;

/// Chrome extension order, as TLS wire IDs. 17 extensions: the 16 of the
/// verified Chrome 147 reference capture plus trust_anchors (51764),
/// which real Chrome 153 always sends (measured live against
/// tls.peet.ws: 19 extensions each handshake = 17 + 2 GREASE).
///
/// **Real Chrome shuffling behavior** (per Fastly TLS Fingerprinting blog
/// + Chromestatus 5124606246518784 + BoringSSL `ssl_setup_extension_permutation`
/// source): Chrome shuffles ALL non-PSK extensions with a single Fisher-Yates
/// pass — there is no documented bucket structure. The only positional
/// constraint is psk_key_exchange_modes / pre_shared_key being last (BoringSSL
/// enforces this). The previous 3-bucket scheme was folklore from earlier
/// public RE work; it reduced shuffle entropy by ~720,000× and put
/// signature_algorithms always at position 16 — a deterministic positional
/// pattern that per-handshake classifiers can detect as anomalous.
const CHROME_EXTENSION_ORDER: &[u16] = &[
    51,                // key_share
    65037,             // encrypted_client_hello
    10,                // supported_groups
    18,                // certificate_timestamp
    45,                // psk_key_exchange_modes
    23,                // extended_master_secret
    17613,             // application_settings_new (ALPS)
    27,                // cert_compression
    43,                // supported_versions
    0,                 // server_name
    65281,             // renegotiate
    11,                // ec_point_formats
    5,                 // status_request
    16,                // application_layer_protocol_negotiation (ALPN)
    35,                // session_ticket
    13,                // signature_algorithms
    EXT_TRUST_ANCHORS, // trust_anchors (0xCA34) — always sent by real Chrome 153
];

/// Trust Anchors extension wire ID (TLSEXT_TYPE_trust_anchors).
/// Real Chrome 153 sends it with an EMPTY anchor list when the server's
/// HTTPS record advertises no `tls-trust-anchors` parameter (the
/// anonymity-set guidance: no per-machine fingerprintable content).
/// Presence alone signals a current Chrome network stack.
const EXT_TRUST_ANCHORS: u16 = 51764;

/// Generate a fresh Fisher-Yates shuffle over the Chrome extension order.
fn shuffled_chrome_extension_order() -> Vec<ExtensionType> {
    let mut rng = rand::rng();
    let mut order: Vec<u16> = CHROME_EXTENSION_ORDER.to_vec();
    order.shuffle(&mut rng);
    order.into_iter().map(ExtensionType::from).collect()
}

/// Fixed wire order as `ExtensionType`s (Safari/Firefox arms).
fn fixed_extension_order(base: &[u16]) -> Vec<ExtensionType> {
    base.iter().copied().map(ExtensionType::from).collect()
}

/// Build an `SslConnector` configured with the TLS fingerprint matching
/// `profile.device_class`. Currently all variants share Chrome 147 desktop
/// configuration; this also branches for Android and iOS Safari.
pub fn chrome_connector(profile: &StealthProfile) -> Result<SslConnector, NetError> {
    // Per-device_class branching.
    //  - Desktop / Android: shared Chrome 147 cipher/sigalg/extension config.
    //    Android only diverges in the curves list (Kyber768Draft00 vs MLKEM).
    //  - MobileIOS: distinct Safari 18 cipher/sigalg/curves + skip Fisher-Yates
    //    extension permutation + zlib cert compression + SslOptions::NO_TICKET.
    //    Per-connection ALPS and ECH grease are also skipped — see
    //    configure_connection() below.
    let is_safari_ios = profile.device_class == DeviceClass::MobileIOS;
    // Firefox wire class: a desktop profile whose browser family is Firefox
    // emits an NSS-class ClientHello (no GREASE, FFDHE groups,
    // delegated_credentials + record_size_limit, fixed extension order)
    // instead of Chrome's. Without this a firefox_135_* profile put a
    // Chrome JA4 under a Firefox UA — an incoherent identity that any JA4↔UA
    // cross-check would flag.
    let is_firefox = profile.browser_name == "Firefox";
    let curves: &str = if is_firefox {
        CURVES_FIREFOX
    } else {
        match profile.device_class {
            DeviceClass::MobileAndroid => CURVES_ANDROID,
            DeviceClass::MobileIOS => CURVES_SAFARI_IOS,
            DeviceClass::Desktop => CURVES_DESKTOP,
        }
    };
    let cipher_list: &str = if is_safari_ios {
        CIPHER_LIST_SAFARI_IOS
    } else if is_firefox {
        CIPHER_LIST_FIREFOX
    } else {
        CIPHER_LIST
    };
    let sigalgs_list: &str = if is_safari_ios {
        SIGALGS_LIST_SAFARI_IOS
    } else if is_firefox {
        SIGALGS_LIST_FIREFOX
    } else {
        SIGALGS_LIST
    };
    let mut builder =
        SslConnector::builder(SslMethod::tls()).map_err(|e| NetError::Tls(e.to_string()))?;

    // Cipher suites (per device_class)
    builder
        .set_cipher_list(cipher_list)
        .map_err(|e| NetError::Tls(e.to_string()))?;

    // Elliptic curves (per device_class)
    builder
        .set_curves_list(curves)
        .map_err(|e| NetError::Tls(e.to_string()))?;

    // Signature algorithms (per device_class)
    builder
        .set_sigalgs_list(sigalgs_list)
        .map_err(|e| NetError::Tls(e.to_string()))?;

    // ALPN
    builder
        .set_alpn_protos(ALPN_PROTOS)
        .map_err(|e| NetError::Tls(e.to_string()))?;

    // TLS version range. Safari iOS 18.x advertises 4 versions (1.0, 1.1,
    // 1.2, 1.3) in supported_versions per reference Safari iOS captures —
    // visible as a length-difference on the extension. Servers still
    // negotiate 1.3 because no real server speaks 1.0/1.1 anymore, but the
    // ClientHello must advertise all four to fingerprint as Safari.
    let min_version = if is_safari_ios {
        SslVersion::TLS1
    } else {
        SslVersion::TLS1_2
    };
    builder
        .set_min_proto_version(Some(min_version))
        .map_err(|e| NetError::Tls(e.to_string()))?;
    builder
        .set_max_proto_version(Some(SslVersion::TLS1_3))
        .map_err(|e| NetError::Tls(e.to_string()))?;

    // GREASE: Chrome sprinkles GREASE across cipher/group/extension lists;
    // NSS-class Firefox sends NONE. The visible no-GREASE shape is itself a
    // Firefox tell, so disable it for the Firefox arm.
    builder.set_grease_enabled(!is_firefox);

    builder.set_permute_extensions(false);

    builder.enable_ocsp_stapling();
    builder.enable_signed_cert_timestamps();

    // Two key shares (X25519MLKEM768 + X25519) are set per-connection in
    // `configure_connection` (btls exposes this on the connection, not
    // the context).

    // Firefox-only extensions: delegated_credentials (0x22) and
    // record_size_limit (0x1c). Both are hard Firefox/NSS signatures absent
    // from every Chrome build. btls exposes them as builder methods.
    if is_firefox {
        builder
            .set_delegated_credentials(FIREFOX_DELEGATED_CREDENTIALS)
            .map_err(|e| NetError::Tls(e.to_string()))?;
        builder.set_record_size_limit(FIREFOX_RECORD_SIZE_LIMIT);
    }

    // Certificate compression. Chrome desktop+Android = Brotli (algo 2).
    // iOS Safari = Zlib (algo 1). Firefox 135 advertises zlib THEN brotli in
    // its compress_certificate ext (NSS order).
    if is_firefox {
        builder
            .add_certificate_compression_algorithm(ZlibCertCompressor)
            .map_err(|e| NetError::Tls(e.to_string()))?;
        builder
            .add_certificate_compression_algorithm(BrotliCertCompressor)
            .map_err(|e| NetError::Tls(e.to_string()))?;
    } else {
        let is_zlib = is_safari_ios;
        if is_zlib {
            builder
                .add_certificate_compression_algorithm(ZlibCertCompressor)
                .map_err(|e| NetError::Tls(e.to_string()))?;
        } else {
            builder
                .add_certificate_compression_algorithm(BrotliCertCompressor)
                .map_err(|e| NetError::Tls(e.to_string()))?;
        }
    }

    // iOS Safari does not send the session_ticket extension at all.
    // SslOptions::NO_TICKET tells BoringSSL to omit the extension entirely
    // (vs sending it with a stale ticket).
    if is_safari_ios {
        builder.set_options(SslOptions::NO_TICKET);
    }

    // Load Mozilla root certificates into the certificate store
    let mut cert_store = X509StoreBuilder::new().map_err(|e| NetError::Tls(e.to_string()))?;
    for cert_der in webpki_root_certs::TLS_SERVER_ROOT_CERTS {
        let x509 = X509::from_der(cert_der.as_ref())
            .map_err(|e| NetError::Tls(format!("failed to parse root cert: {e}")))?;
        let _ = cert_store.add_cert(x509);
    }
    add_env_root_certs(&mut cert_store)?;
    builder.set_cert_store(cert_store.build());

    // Extension order (a context knob — applied pre-build like the rest):
    //  - Chrome: per-handshake Fisher-Yates shuffle of all 17 desktop extensions
    //  - Safari iOS: FIXED order (same every handshake)
    //  - Firefox: FIXED NSS order (same every handshake)
    // PADDING positional ordering still requires raw extension injection
    // (deferred); BoringSSL auto-emits PADDING when ClientHello length
    // crosses ~512 bytes.
    let order = if is_safari_ios {
        fixed_extension_order(SAFARI_IOS_EXTENSION_ORDER)
    } else if is_firefox {
        // Firefox/NSS emits a FIXED extension order every handshake (no
        // Fisher-Yates) — use the Firefox order verbatim.
        fixed_extension_order(FIREFOX_EXTENSION_ORDER)
    } else {
        shuffled_chrome_extension_order()
    };
    builder
        .set_extension_permutation(&order)
        .map_err(|e| NetError::Tls(e.to_string()))?;

    let connector = builder.build();

    // Trust Anchors (51764) — Chrome arms send the extension with an EMPTY
    // anchor list (the server advertised no `tls-trust-anchors` parameter,
    // so per the anonymity-set guidance there is nothing fingerprintable
    // to send; presence alone signals a current Chrome network stack).
    // Safari/Firefox arms omit it — no evidence they send it.
    //
    // Declared manually: btls-sys binds the symbol from its vendored
    // BoringSSL, but exposes no safe wrapper, so declare the C ABI
    // directly — it links against the same static BoringSSL.
    // Signature: int SSL_CTX_set1_requested_trust_anchors(SSL_CTX *ctx,
    // const uint8_t *ids, size_t ids_len); returns 1 on success.
    extern "C" {
        fn SSL_CTX_set1_requested_trust_anchors(
            ctx: *mut btls_sys::SSL_CTX,
            ids: *const u8,
            ids_len: usize,
        ) -> std::os::raw::c_int;
    }
    if !is_safari_ios && !is_firefox {
        // SAFETY: the function copies `len` bytes from `ids` into the
        // SSL_CTX. An empty list passes a null pointer with length 0, which
        // BoringSSL accepts (Chromium itself passes an empty vector for
        // exactly this case). `connector.context()` is a live context we
        // just built. Return value 1 = success.
        let rc = unsafe {
            SSL_CTX_set1_requested_trust_anchors(connector.context().as_ptr(), std::ptr::null(), 0)
        };
        if rc != 1 {
            return Err(NetError::Tls(
                "failed to set requested trust anchors".into(),
            ));
        }
    }

    Ok(connector)
}

/// Configure a per-connection TLS session with ALPS, ECH GREASE, and SNI.
/// Per-`profile.device_class` branching:
///  - Desktop / Android: ECH grease + ALPS HTTP/2 SETTINGS payload
///  - MobileIOS: skip BOTH (Safari has neither)
pub fn configure_connection(
    connector: &SslConnector,
    profile: &StealthProfile,
    domain: &str,
) -> Result<ConnectConfiguration, NetError> {
    let mut config = connector
        .configure()
        .map_err(|e| NetError::Tls(e.to_string()))?;

    let is_safari_ios = profile.device_class == DeviceClass::MobileIOS;
    let is_firefox = profile.browser_name == "Firefox";

    if !is_safari_ios {
        // ECH GREASE — Chrome desktop+Android AND Firefox all send it.
        // Safari does not.
        config.set_enable_ech_grease(true);
    }

    if !is_safari_ios && !is_firefox {
        // Application-layer settings (ALPS) for HTTP/2.
        // Chrome 147 Headless sends 4 settings: 1, 2, 4, 6.
        // Safari has no ALPS extension at all — skip entirely on iOS.
        // Firefox has no ALPS extension either — skip for the Firefox arm.
        let alps_payload: &[u8] = &[
            // SETTINGS frame (Length 24, Type 4, Flags 0, Stream 0)
            0x00, 0x00, 0x18, 0x04, 0x00, 0x00, 0x00, 0x00, 0x00, // ID 1: 65536
            0x00, 0x01, 0x00, 0x01, 0x00, 0x00, // ID 2: 0
            0x00, 0x02, 0x00, 0x00, 0x00, 0x00, // ID 4: 6291456
            0x00, 0x04, 0x00, 0x60, 0x00, 0x00, // ID 6: 262144
            0x00, 0x06, 0x00, 0x04, 0x00, 0x00,
            // Empty ACCEPT_CH frame (Length 0, Type 0x89, Flags 0, Stream 0)
            0x00, 0x00, 0x00, 0x89, 0x00, 0x00, 0x00, 0x00, 0x00,
        ];

        // SAFETY: BoringSSL's `SSL_add_application_settings` reads the
        // ALPN name (`b"h2"`, length 2) and the ALPS payload buffer
        // (`alps_payload` — a contiguous static slice we built above);
        // both are valid, contiguous, non-null, and live for the
        // entire call. `config.as_ptr()` returns a non-null pointer
        // to the live `SslContext` we own here. BoringSSL only reads
        // these buffers; it copies the data into the SSL_CTX, no
        // ownership transfer.
        unsafe {
            if btls_sys::SSL_add_application_settings(
                config.as_ptr(),
                b"h2".as_ptr(),
                2,
                alps_payload.as_ptr(),
                alps_payload.len(),
            ) != 1
            {
                return Err(NetError::Tls("failed to add ALPS settings".into()));
            }
        }
        config.set_alps_use_new_codepoint(true);
    }

    // Two key shares — Chrome 131+ and Firefox 132+ send X25519MLKEM768 +
    // X25519; Safari (no PQ groups configured) sends X25519 + P-256, the
    // first two of its supported groups (this mirrors the old
    // set_key_shares_limit(2) behavior; a share for an unconfigured group
    // fails setup and no ClientHello goes out at all).
    let shares: &[KeyShare] = if is_safari_ios {
        &[KeyShare::X25519, KeyShare::P256]
    } else {
        &[KeyShare::X25519_MLKEM768, KeyShare::X25519]
    };
    config
        .set_client_key_shares(shares)
        .map_err(|e| NetError::Tls(e.to_string()))?;

    // SNI is the same for all profiles.
    let sni_domain = domain.trim_start_matches('[').trim_end_matches(']');
    if sni_domain.parse::<std::net::IpAddr>().is_ok() {
        config.set_use_server_name_indication(false);
    } else {
        config
            .set_hostname(sni_domain)
            .map_err(|e| NetError::Tls(e.to_string()))?;
    }

    Ok(config)
}

/// Establish a TLS connection to `domain` using the provided `SslConnector`.
pub async fn connect_tls(
    connector: &SslConnector,
    profile: &StealthProfile,
    domain: &str,
    stream: TcpStream,
) -> Result<SslStream<TcpStream>, NetError> {
    let config = configure_connection(connector, profile, domain)?;
    let sni_domain = domain.trim_start_matches('[').trim_end_matches(']');
    let ssl = config
        .into_ssl(sni_domain)
        .map_err(|e| NetError::Tls(format!("TLS config failed: {e}")))?;
    let mut stream = SslStream::new(ssl, stream)
        .map_err(|e| NetError::Tls(format!("TLS stream failed: {e}")))?;
    std::pin::Pin::new(&mut stream)
        .connect()
        .await
        .map_err(|e| NetError::Tls(format!("TLS handshake failed: {e}")))?;
    Ok(stream)
}

/// Adds the PEM roots named by `SSL_CERT_FILE` to `store`, the same variable
/// OpenSSL and curl read. A TLS-intercepting proxy signs every server
/// certificate with its own CA, which the embedded Mozilla roots do not hold.
fn add_env_root_certs(store: &mut X509StoreBuilder) -> Result<(), NetError> {
    let Some(path) = std::env::var_os("SSL_CERT_FILE") else {
        return Ok(());
    };
    let pem = std::fs::read(&path).map_err(|e| {
        NetError::Tls(format!(
            "failed to read SSL_CERT_FILE {}: {e}",
            path.to_string_lossy()
        ))
    })?;
    add_pem_root_certs(store, &pem)
}

fn add_pem_root_certs(store: &mut X509StoreBuilder, pem: &[u8]) -> Result<(), NetError> {
    let certs = X509::stack_from_pem(pem)
        .map_err(|e| NetError::Tls(format!("failed to parse SSL_CERT_FILE: {e}")))?;
    if certs.is_empty() {
        return Err(NetError::Tls(
            "SSL_CERT_FILE holds no PEM certificate".to_string(),
        ));
    }
    for cert in certs {
        store
            .add_cert(cert)
            .map_err(|e| NetError::Tls(format!("failed to add SSL_CERT_FILE root: {e}")))?;
    }
    Ok(())
}

/// Returns the negotiated ALPN protocol from a TLS stream, if any.
pub fn negotiated_alpn(stream: &SslStream<TcpStream>) -> Option<&[u8]> {
    stream.ssl().selected_alpn_protocol()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A proxy CA bundle often repeats a Mozilla root, so a root already in
    /// the store must not fail the connector; garbage must.
    #[test]
    fn pem_root_certs_accept_duplicates_and_reject_garbage() {
        let der = webpki_root_certs::TLS_SERVER_ROOT_CERTS[0].as_ref();
        let pem = X509::from_der(der).unwrap().to_pem().unwrap();
        let mut store = X509StoreBuilder::new().unwrap();
        store.add_cert(X509::from_der(der).unwrap()).unwrap();
        add_pem_root_certs(&mut store, &pem).unwrap();
        assert!(add_pem_root_certs(&mut store, b"not a certificate").is_err());
    }

    /// Self-verifying JA4 drift guard + UA/TLS coherence assert.
    /// Network-free.
    ///
    /// Pins every JA4 input (cipher list, sigalg list, supported-groups
    /// order, extension count) byte-/element-exact to the verified-real
    /// Chrome reference so the fingerprint can never silently drift
    /// again (any edit to
    /// the constants fails this test loudly), and machine-checks that
    /// the deliberate UA=148 / TLS-ref=147 split is the documented,
    /// wire-coherent one (see [`TLS_CHROME_MAJOR`] docs).
    #[test]
    fn tls_fingerprint_vectors_no_silent_drift() {
        // --- JA4 input 1: cipher suites (order is JA4-significant) ---
        const EXPECT_CIPHERS: &str = "TLS_AES_128_GCM_SHA256:TLS_AES_256_GCM_SHA384:\
TLS_CHACHA20_POLY1305_SHA256:TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256:\
TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256:TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384:\
TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384:TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256:\
TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256:TLS_ECDHE_RSA_WITH_AES_128_CBC_SHA:\
TLS_ECDHE_RSA_WITH_AES_256_CBC_SHA:TLS_RSA_WITH_AES_128_GCM_SHA256:\
TLS_RSA_WITH_AES_256_GCM_SHA384:TLS_RSA_WITH_AES_128_CBC_SHA:\
TLS_RSA_WITH_AES_256_CBC_SHA";
        assert_eq!(
            CIPHER_LIST, EXPECT_CIPHERS,
            "Chrome cipher list drifted from the verified-real reference \
             — JA4 cipher hash would change"
        );

        // --- JA4 input 2: signature algorithms (order is JA4-significant) ---
        // Chrome 153 advertises ML-DSA-44/65/87 first (measured live), so the
        // eight classical algorithms alone are NOT the Chrome list.
        const EXPECT_SIGALGS: &str = "mldsa44:mldsa65:mldsa87:\
ecdsa_secp256r1_sha256:rsa_pss_rsae_sha256:\
rsa_pkcs1_sha256:ecdsa_secp384r1_sha384:rsa_pss_rsae_sha384:rsa_pkcs1_sha384:\
rsa_pss_rsae_sha512:rsa_pkcs1_sha512";
        assert_eq!(
            SIGALGS_LIST, EXPECT_SIGALGS,
            "Chrome sigalg list drifted — JA4 sigalg hash would change"
        );

        // --- JA4 input 3: supported groups / curves order ---
        assert_eq!(
            CURVES_DESKTOP, "X25519MLKEM768:X25519:P-256:P-384",
            "Chrome desktop curve order drifted (post-quantum MLKEM768 \
             must lead) — JA4 supported_groups would change"
        );

        // --- JA4 input 4: extension count (17 — JA4 `c` digit) ---
        assert_eq!(
            CHROME_EXTENSION_ORDER.len(),
            17,
            "Chrome extension count drifted — JA4 extension-count digit \
             would change"
        );

        // --- UA / TLS coherence: both advertise the same major ---
        assert_eq!(TLS_CHROME_MAJOR, 153);
        assert_eq!(UA_CHROME_MAJOR, 153);
        // The hello is 153-class (trust_anchors present); see
        // TLS_CHROME_MAJOR docs. JA4 cannot encode the Chrome version,
        // so a JA4-vs-UA cross-check verifies the family only.

        fn ua_chrome_major(ua: &str) -> Option<u32> {
            let i = ua.find("Chrome/")? + "Chrome/".len();
            ua[i..].split('.').next()?.parse().ok()
        }

        for profile in [
            crate::stealth::presets::chrome_153_macos(),
            crate::stealth::presets::chrome_153_windows(),
        ] {
            assert_eq!(
                ua_chrome_major(&profile.user_agent),
                Some(UA_CHROME_MAJOR),
                "desktop Chrome preset UA major must equal UA_CHROME_MAJOR \
                 (the coherence single-source-of-truth); UA was {:?}",
                profile.user_agent
            );
            assert_eq!(
                profile.tls_impersonate, "chrome_147",
                "desktop Chrome preset TLS profile must be the verified-real \
                 chrome_147 reference (wire-equivalent to Chrome \
                 {UA_CHROME_MAJOR}); see TLS_CHROME_MAJOR docs"
            );
        }
    }

    /// Capture the first 5 bytes of our outbound ClientHello (the TLS
    /// record header) and assert the record version is 0x0301 (TLS 1.0).
    /// Source-code analysis of `boringssl/src/ssl/ssl_aead_ctx.cc:168-173`
    /// confirms `RecordVersion()` returns `TLS1_VERSION` (0x0301) for the
    /// initial ClientHello (null cipher, version_ == 0). This test verifies
    /// it empirically — a BoringSSL source patch for the TLS 1.0 record
    /// version is **NOT NEEDED**.
    #[tokio::test]
    async fn safari_ios_emits_tls_1_0_record_version() {
        use tokio::io::AsyncReadExt;
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Background server that just reads the first 5 bytes and reports.
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                stream.read_exact(&mut buf),
            )
            .await
            .unwrap()
            .unwrap();
            buf
        });

        // Connect with iOS Safari profile.
        let profile = crate::stealth::presets::iphone_15_pro_safari_18();
        let connector = chrome_connector(&profile).expect("connector");
        let tcp = TcpStream::connect(addr).await.unwrap();
        // We expect the handshake to fail (server doesn't respond), but the
        // ClientHello is sent before that. Race the timeout against the
        // server's read.
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            connect_tls(&connector, &profile, "localhost", tcp),
        )
        .await;

        let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("server timeout")
            .expect("server task");

        let content_type = bytes[0];
        let record_version = ((bytes[1] as u16) << 8) | (bytes[2] as u16);

        // Content type 22 = TLS handshake
        assert_eq!(
            content_type, 22,
            "expected TLS handshake (22), got {content_type}"
        );

        // Record version: real Safari sends 0x0301 (TLS 1.0); BoringSSL
        // emits the same for null-cipher (initial ClientHello).
        assert_eq!(
            record_version, 0x0301,
            "iOS Safari record version: got 0x{record_version:04x}, expected 0x0301 (TLS 1.0). \
             If this is 0x0303 then a BoringSSL source patch IS needed; if 0x0301 then \
             our current build already matches Safari."
        );
    }

    /// Same record-version check for desktop Chrome profile. Real Chrome
    /// also sends 0x0301 (TLS 1.0) record version for the initial ClientHello
    /// — TLS-version selection happens in the inner extension, not the outer
    /// record header. This test confirms the BoringSSL behavior is uniform
    /// across desktop and Safari profiles.
    #[tokio::test]
    async fn desktop_chrome_emits_tls_1_0_record_version() {
        use tokio::io::AsyncReadExt;
        use tokio::net::{TcpListener, TcpStream};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            tokio::time::timeout(
                std::time::Duration::from_secs(3),
                stream.read_exact(&mut buf),
            )
            .await
            .unwrap()
            .unwrap();
            buf
        });

        let profile = crate::stealth::presets::chrome_153_macos();
        let connector = chrome_connector(&profile).expect("connector");
        let tcp = TcpStream::connect(addr).await.unwrap();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            connect_tls(&connector, &profile, "localhost", tcp),
        )
        .await;

        let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), server)
            .await
            .expect("server timeout")
            .expect("server task");

        let record_version = ((bytes[1] as u16) << 8) | (bytes[2] as u16);
        assert_eq!(
            record_version, 0x0301,
            "Chrome desktop record version: got 0x{record_version:04x}, expected 0x0301."
        );
    }

    #[test]
    fn test_shuffle_is_full_fisher_yates() {
        // Real Chrome shuffles all 17 extensions uniformly (no buckets).
        // Verify the shuffle preserves the full set + is non-deterministic.
        let p1 = shuffled_chrome_extension_order();
        let p2 = shuffled_chrome_extension_order();

        assert_eq!(p1.len(), 17);
        assert_eq!(p2.len(), 17);

        let mut sorted: Vec<String> = p1.iter().map(|e| format!("{e:?}")).collect();
        sorted.sort();
        let mut expected: Vec<String> = CHROME_EXTENSION_ORDER
            .iter()
            .map(|id| format!("{:?}", ExtensionType::from(*id)))
            .collect();
        expected.sort();
        assert_eq!(sorted, expected, "shuffle must preserve the set");

        // Probabilistically should differ run-to-run.
        assert_ne!(p1, p2, "Shuffle should be non-deterministic");
    }
}
