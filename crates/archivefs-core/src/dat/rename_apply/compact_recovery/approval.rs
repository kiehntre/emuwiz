//! Isolated synthetic issuer protocol, NOT a production authentication service.
//! Public-key and directory identity are trusted configuration, never journal data.
use super::*;
use ring::signature::{ED25519, UnparsedPublicKey};
#[cfg(test)]
use ring::signature::{Ed25519KeyPair, KeyPair};
use std::io::Write;

const LIMIT: u64 = 16 * 1024;
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verifier {
    pub(super) directory: PathBuf,
    pub(super) identity: (u64, u64),
    pub(super) public_key: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Signed {
    payload: Vec<u8>,
    signature: Vec<u8>,
}
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Context {
    pub(super) verifier: Verifier,
    signed: Signed,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Boundary {
    format: String,
    ticket: Ticket,
    image: [u8; 32],
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Grant {
    format: String,
    nonce: String,
    context: Signed,
    action: RequestedAction,
    observations: [u8; 32],
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Approval {
    pub(super) context: Context,
    pub(super) nonce: String,
}
// Only the fixture issuer owns signing material. Recovery accepts public context.
#[cfg(test)]
pub(super) struct Issuer {
    pub(super) verifier: Verifier,
    key: Ed25519KeyPair,
    original: Vec<u8>,
    ticket: Ticket,
}
fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
#[cfg(test)]
fn signed<T: Serialize>(key: &Ed25519KeyPair, value: &T) -> io::Result<Signed> {
    let payload = serde_json::to_vec(value).map_err(fail)?;
    if payload.is_empty() || payload.len() as u64 > LIMIT {
        return Err(fail("issuer payload limit"));
    }
    let signature = key.sign(&payload).as_ref().to_vec();
    Ok(Signed { payload, signature })
}
fn verify<T: serde::de::DeserializeOwned>(v: &Verifier, s: &Signed) -> io::Result<T> {
    if s.payload.is_empty() || s.payload.len() as u64 > LIMIT {
        return Err(fail("empty/oversized trusted context"));
    }
    UnparsedPublicKey::new(&ED25519, &v.public_key)
        .verify(&s.payload, &s.signature)
        .map_err(|_| fail("issuer signature invalid; journal cannot authenticate itself"))?;
    serde_json::from_slice(&s.payload).map_err(fail)
}
impl Verifier {
    fn open(&self) -> io::Result<File> {
        if std::fs::canonicalize(&self.directory)? != self.directory {
            return Err(fail("approval directory is not canonical"));
        }
        let f = open_dir(&self.directory)?;
        private(&f)?;
        if identity(&f)? != self.identity {
            return Err(fail("approval directory identity changed"));
        }
        Ok(f)
    }
    fn bound(&self, f: &File) -> io::Result<()> {
        if identity(f)? != identity(&self.open()?)? {
            return Err(fail("approval directory rebound"));
        }
        Ok(())
    }
}
fn read_record(dir: &File, name: &str) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(at(dir, name))?;
    private(&f)?;
    let m = f.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.len() > LIMIT {
        return Err(fail("invalid approval record"));
    }
    let mut bytes = Vec::new();
    std::io::Read::by_ref(&mut f)
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > LIMIT {
        return Err(fail("approval record grew"));
    }
    Ok(bytes)
}
fn publish(dir: &File, name: &str, bytes: &[u8]) -> io::Result<()> {
    if bytes.len() as u64 > LIMIT {
        return Err(fail("approval size limit"));
    }
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(at(dir, name))?;
    f.write_all(bytes)?;
    f.sync_all()?;
    dir.sync_all()
}
impl Context {
    pub(super) fn require_issuer(&self, trusted: &Verifier) -> io::Result<()> {
        if &self.verifier != trusted {
            return Err(fail("grant cannot select its own issuer or store"));
        }
        trusted.open()?;
        Ok(())
    }
    pub(super) fn ticket(&self, trusted: &Verifier) -> io::Result<Ticket> {
        self.require_issuer(trusted)?;
        let dir = self.verifier.open()?;
        let boundary: Boundary = verify(&self.verifier, &self.signed)?;
        if boundary.format != "emuwiz-test-boundary-v1" {
            return Err(fail("unknown boundary format"));
        }
        self.verifier.bound(&dir)?;
        Ok(boundary.ticket)
    }
    pub(super) fn check(
        &self,
        trusted: &Verifier,
        store: &Store,
        base: &Authority,
    ) -> io::Result<()> {
        let boundary: Boundary = verify(&self.verifier, &self.signed)?;
        self.ticket(trusted)?;
        store.bound()?;
        if serde_json::to_vec(&boundary.ticket).map_err(fail)?
            != serde_json::to_vec(&store.ticket).map_err(fail)?
            || base.binding() != boundary.ticket.binding
        {
            return Err(fail("foreign recovery boundary"));
        }
        let bytes = container_bytes(store)?;
        if digest(&bytes) != boundary.image {
            return Err(fail("evidence differs from independently issued boundary"));
        }
        store.bound()
    }
}
#[cfg(test)]
impl Issuer {
    /// Called only by the separate fixture controller using its original plan.
    /// Never constructed by a journal decoder or production recovery caller.
    pub(super) fn new(
        directory: &Path,
        original: Vec<u8>,
        ticket: Ticket,
        key_bytes: &[u8],
    ) -> io::Result<Self> {
        let key = Ed25519KeyPair::from_pkcs8(key_bytes).map_err(|_| fail("issuer key invalid"))?;
        let dir = open_dir(directory)?;
        private(&dir)?;
        Ok(Self {
            verifier: Verifier {
                directory: directory.into(),
                identity: identity(&dir)?,
                public_key: key.public_key().as_ref().to_vec(),
            },
            key,
            original,
            ticket,
        })
    }
    pub(super) fn context(&self, store: &Store, base: &Authority) -> io::Result<Context> {
        if base.original_bytes() != self.original
            || serde_json::to_vec(&store.ticket).map_err(fail)?
                != serde_json::to_vec(&self.ticket).map_err(fail)?
        {
            return Err(fail(
                "issuer rejects journal outside independently registered plan",
            ));
        }
        store.bound()?;
        let image = digest(&container_bytes(store)?);
        let boundary = Boundary {
            format: "emuwiz-test-boundary-v1".into(),
            ticket: self.ticket.clone(),
            image,
        };
        Ok(Context {
            verifier: self.verifier.clone(),
            signed: signed(&self.key, &boundary)?,
        })
    }
    pub(super) fn issue(
        &self,
        context: &Context,
        action: RequestedAction,
        observations: &[u8],
    ) -> io::Result<Approval> {
        if context.verifier.public_key != self.verifier.public_key {
            return Err(fail("foreign issuer"));
        }
        let _: Boundary = verify(&self.verifier, &context.signed)?;
        let mut random = [0u8; 32];
        getrandom::fill(&mut random).map_err(fail)?;
        let nonce = random
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        let grant = Grant {
            format: "emuwiz-test-grant-v1".into(),
            nonce: nonce.clone(),
            context: context.signed.clone(),
            action,
            observations: digest(observations),
        };
        let record = serde_json::to_vec(&signed(&self.key, &grant)?).map_err(fail)?;
        let dir = self.verifier.open()?;
        publish(&dir, &format!("grant-{nonce}"), &record)?;
        self.verifier.bound(&dir)?;
        Ok(Approval {
            context: context.clone(),
            nonce,
        })
    }
}
impl Approval {
    /// Any existing tombstone (including partial/corrupt writes) means spent.
    /// This protocol never renames or unlinks records. External deletion/store
    /// rollback is outside this participating-fixture contract. Missing grant fails closed.
    pub(super) fn consume(
        &self,
        trusted: &Verifier,
        action: RequestedAction,
        observations: &[u8],
    ) -> io::Result<()> {
        self.context.require_issuer(trusted)?;
        if self.nonce.len() != 64 || !self.nonce.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(fail("invalid grant id"));
        }
        let v = &self.context.verifier;
        let dir = v.open()?;
        let bytes = read_record(&dir, &format!("grant-{}", self.nonce))?;
        let signed_grant: Signed = serde_json::from_slice(&bytes).map_err(fail)?;
        let grant: Grant = verify(v, &signed_grant)?;
        if grant.format != "emuwiz-test-grant-v1" || grant.nonce != self.nonce {
            return Err(fail("foreign grant"));
        }
        // Burn BEFORE evidence/action/identity checks. Failures cannot recycle it.
        publish(&dir, &format!("used-{}", self.nonce), &digest(&bytes)).map_err(|e| {
            if e.kind() == io::ErrorKind::AlreadyExists {
                fail("grant already consumed")
            } else {
                e
            }
        })?;
        v.bound(&dir)?;
        if grant.action != action
            || grant.observations != digest(observations)
            || serde_json::to_vec(&grant.context).map_err(fail)?
                != serde_json::to_vec(&self.context.signed).map_err(fail)?
        {
            return Err(fail(
                "consumed grant differs from requested evidence/action/identity",
            ));
        }
        Ok(())
    }
}

fn container_bytes(store: &Store) -> io::Result<Vec<u8>> {
    store.bound()?;
    store.allocation()?;
    let mut bytes = vec![0; usize::try_from(store.layout.length).map_err(fail)?];
    store.file.read_exact_at(&mut bytes, 0)?;
    store.bound()?;
    Ok(bytes)
}
