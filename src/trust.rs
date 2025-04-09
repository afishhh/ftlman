use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use speedy_xml::reader::Event;

use crate::util::hex_to_byte_array;

#[derive(Debug, Clone, Copy)]
pub enum ManifestVersion {
    One = 1,
}

#[derive(Debug, Clone, Copy)]
pub enum KeyId {
    Test,
}

impl KeyId {
    pub fn name(self) -> &'static str {
        match self {
            KeyId::Test => "Development Test Key",
        }
    }
}

#[derive(Clone)]
pub struct TrustManifest {
    version: ManifestVersion,
    key_id: KeyId,
    trusted_files: HashMap<Box<str>, [u8; 32]>,
}

mod keys;

impl TrustManifest {
    pub fn parse_and_verify(mut reader: speedy_xml::Reader, signature: &str) -> Result<TrustManifest> {
        let mut result = loop {
            {
                match reader.next().transpose()? {
                    Some(Event::Start(start)) if start.prefix().is_none() && start.name() == "trustManifest" => {
                        let Some(version_attr) = start.attributes().find(|x| x.name() == "version") else {
                            bail!(r#"Missing "version" attribute"#)
                        };
                        let Some(key_attr) = start.attributes().find(|x| x.name() == "key") else {
                            bail!(r#"Missing "key" attribute"#)
                        };

                        let key_id = match key_attr.value().trim() {
                            "test" => KeyId::Test,
                            key => bail!("Unknown key id {}", key.escape_default()),
                        };

                        Self::verify(key_id, reader.buffer(), signature).context("Failed to establish trust")?;

                        break TrustManifest {
                            version: match version_attr.value().trim() {
                                "1" => ManifestVersion::One,
                                ver => bail!("Unknown manifest version {}", ver.escape_default()),
                            },
                            key_id,
                            trusted_files: HashMap::new(),
                        };
                    }
                    Some(Event::Text(text)) if text.raw_content().bytes().all(|b| b.is_ascii_whitespace()) => (),
                    Some(event) => {
                        bail!("Unexpected top-level event {event:?}")
                    }
                    None => bail!("Unexpected EOF before root element"),
                }
            }
        };

        loop {
            match reader.next().transpose()? {
                Some(Event::Empty(start)) => match start.name() {
                    "file" => {
                        let Some(blake2_attr) = start.attributes().find(|x| x.name() == "blake2s") else {
                            bail!(r#"File tag missing "blake2s" attribute"#)
                        };
                        let Some(path_attr) = start.attributes().find(|x| x.name() == "path") else {
                            bail!(r#"File tag missing "path" attribute"#)
                        };

                        result.trusted_files.insert(
                            path_attr.value().into(),
                            hex_to_byte_array(&blake2_attr.value()).context(r#"Failed to parse "blake2s" hash"#)?,
                        );
                    }
                    start => bail!("Unexpected element in manifest {start:?}"),
                },
                Some(Event::End(end)) if end.prefix().is_none() && end.name() == "trustManifest" => {
                    break;
                }
                Some(Event::Text(text)) if text.raw_content().bytes().all(|b| b.is_ascii_whitespace()) => (),
                Some(event) => {
                    bail!("Unexpected event while parsing manifest content {event:?}")
                }
                None => bail!("Unexpected EOF while parsing manifest content"),
            }
        }

        loop {
            {
                match reader.next().transpose()? {
                    Some(Event::Text(text)) if text.raw_content().bytes().all(|b| b.is_ascii_whitespace()) => (),
                    Some(event) => {
                        bail!("Unexpected trailing top-level event {event:?}")
                    }
                    None => return Ok(result),
                }
            }
        }
    }

    #[allow(unused_variables)]
    fn verify(key: KeyId, text: &str, signature: &str) -> Result<()> {
        let signature =
            ed25519::Signature::from_bytes(&hex_to_byte_array(signature).context("Failed to parse signature as hex")?);

        let key_bytes: &[u8; 32] = match key {
            KeyId::Test => {
                #[cfg(feature = "insecure-trust-test-key")]
                {
                    keys::TEST_PUBLIC_KEY
                }
                #[cfg(not(feature = "insecure-trust-test-key"))]
                bail!("Manifest signed with development-only test key");
            }
        };

        #[allow(unreachable_code)]
        ed25519_dalek::VerifyingKey::from_bytes(key_bytes)
            .context("Failed to decompress public key")?
            .verify_strict(text.as_bytes(), &signature)
            .context("Signature verification failed")
    }

    pub fn version(&self) -> ManifestVersion {
        self.version
    }

    pub fn trusted_files(&self) -> impl ExactSizeIterator<Item = (&str, &[u8; 32])> {
        self.trusted_files.iter().map(|(p, v)| (&**p, v))
    }

    pub fn key(&self) -> KeyId {
        self.key_id
    }
}
