//! Local certificate authority and on-demand leaf certificates for every Doorman host.

use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{Arc, Mutex, RwLock},
};

use anyhow::{Context, Result};
use chrono::{Datelike, Duration, Utc};
use doorman_core::{CA_COMMON_NAME, ca_cert_path, ca_key_path, split_host};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::{
    ServerConfig,
    pki_types::PrivateKeyDer,
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};

use crate::state::Settings;

/// Apple rejects TLS server certificates valid for longer than 398 days.
const LEAF_VALIDITY_DAYS: i64 = 397;
const CA_VALIDITY_DAYS: i64 = 3650;

pub struct Ca {
    issuer: Issuer<'static, KeyPair>,
    leaves: Mutex<HashMap<String, Arc<CertifiedKey>>>,
}

impl Ca {
    /// Loads the CA from the state directory, creating it on first run.
    pub fn load_or_create() -> Result<Self> {
        let (cert_path, key_path) = (ca_cert_path(), ca_key_path());
        let key = if key_path.exists() && cert_path.exists() {
            KeyPair::from_pem(&fs::read_to_string(&key_path)?).context("read Doorman CA key")?
        } else {
            let key = KeyPair::generate().context("generate Doorman CA key")?;
            let certificate = ca_params().self_signed(&key)?;
            write_private(&key_path, key.serialize_pem().as_bytes())?;
            fs::write(&cert_path, certificate.pem()).context("write Doorman CA certificate")?;
            key
        };
        // Leaf signatures only depend on the issuer's name and key, so rebuilding the
        // parameters reproduces the original CA without parsing its certificate.
        Ok(Self {
            issuer: Issuer::new(ca_params(), key),
            leaves: Mutex::new(HashMap::new()),
        })
    }

    fn leaf(&self, host: &str) -> Result<Arc<CertifiedKey>> {
        if let Some(leaf) = self.leaves.lock().expect("leaf cache").get(host) {
            return Ok(leaf.clone());
        }
        let key = KeyPair::generate()?;
        let mut params = CertificateParams::new(vec![host.to_owned()])?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        params.use_authority_key_identifier_extension = true;
        set_validity(&mut params, LEAF_VALIDITY_DAYS);
        let certificate = params.signed_by(&key, &self.issuer)?;

        let private_key = PrivateKeyDer::Pkcs8(key.serialize_der().into());
        let signing_key = rustls::crypto::ring::sign::any_supported_type(&private_key)?;
        let leaf = Arc::new(CertifiedKey::new(
            vec![certificate.der().clone()],
            signing_key,
        ));
        self.leaves
            .lock()
            .expect("leaf cache")
            .insert(host.to_owned(), leaf.clone());
        Ok(leaf)
    }
}

fn ca_params() -> CertificateParams {
    let mut params = CertificateParams::default();
    let mut name = DistinguishedName::new();
    name.push(DnType::CommonName, CA_COMMON_NAME);
    name.push(DnType::OrganizationName, "Doorman");
    params.distinguished_name = name;
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    set_validity(&mut params, CA_VALIDITY_DAYS);
    params
}

fn set_validity(params: &mut CertificateParams, days: i64) {
    let at = |date: chrono::DateTime<Utc>| {
        rcgen::date_time_ymd(date.year(), date.month() as u8, date.day().min(28) as u8)
    };
    let now = Utc::now();
    params.not_before = at(now - Duration::days(1));
    params.not_after = at(now + Duration::days(days));
}

fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("write {}", path.display()))?;
    file.write_all(contents)?;
    Ok(())
}

/// Issues certificates only for hosts Doorman actually routes.
#[derive(Clone)]
struct HostResolver {
    ca: Arc<Ca>,
    settings: Arc<RwLock<Settings>>,
}

impl std::fmt::Debug for HostResolver {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("HostResolver")
    }
}

impl ResolvesServerCert for HostResolver {
    fn resolve(&self, client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let host = client_hello.server_name()?.to_ascii_lowercase();
        let domains = self.settings.read().expect("settings").domains.clone();
        split_host(&host, &domains)?;
        match self.ca.leaf(&host) {
            Ok(leaf) => Some(leaf),
            Err(error) => {
                eprintln!("tls: could not issue a certificate for {host}: {error:#}");
                None
            }
        }
    }
}

pub fn server_config(ca: Arc<Ca>, settings: Arc<RwLock<Settings>>) -> Result<Arc<ServerConfig>> {
    let mut config =
        ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(HostResolver { ca, settings }));
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(Arc::new(config))
}
