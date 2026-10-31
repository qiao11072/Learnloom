use crate::{id, Result};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub config_revision: u64,
    pub preferences_revision: u64,
    pub endpoint: String,
    pub model: String,
    pub credential_ref: Option<String>,
    pub language: String,
    pub context: usize,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            config_revision: 1,
            preferences_revision: 1,
            endpoint: "https://api.deepseek.com".into(),
            model: "deepseek-flash".into(),
            credential_ref: None,
            language: "zh-CN".into(),
            context: 4096,
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Settings {
    pub config_revision: u64,
    pub current: Snapshot,
    pub versions: BTreeMap<u64, Snapshot>,
    pub active_check: Option<String>,
    pub check_state: String,
}
impl Default for Settings {
    fn default() -> Self {
        let current = Snapshot::default();
        Self {
            config_revision: 1,
            versions: BTreeMap::from([(1, current.clone())]),
            current,
            active_check: None,
            check_state: "untested".into(),
        }
    }
}
impl Settings {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension(format!("{}.tmp", id()));
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
    pub fn replace(&mut self, endpoint: &str, model: &str, credential_ref: Option<String>) {
        self.config_revision += 1;
        self.current.config_revision = self.config_revision;
        self.current.endpoint = endpoint.into();
        self.current.model = model.into();
        self.current.credential_ref = credential_ref;
        self.versions
            .insert(self.config_revision, self.current.clone());
        self.active_check = None;
        self.check_state = "untested".into();
    }
    pub fn language(&mut self, language: &str) {
        self.current.preferences_revision += 1;
        self.current.language = language.into();
    }
    pub fn begin_check(&mut self) -> (u64, String) {
        let check = id();
        self.active_check = Some(check.clone());
        self.check_state = "testing".into();
        (self.config_revision, check)
    }
    pub fn finish_check(&mut self, rev: u64, check: &str, passed: bool) -> bool {
        if rev != self.config_revision
            || self.active_check.as_deref() != Some(check)
            || self.check_state != "testing"
        {
            return false;
        }
        self.check_state = if passed { "passed" } else { "failed" }.into();
        true
    }
    pub fn cancel_check(&mut self, check: &str) -> bool {
        if self.active_check.as_deref() != Some(check) || self.check_state != "testing" {
            return false;
        }
        self.check_state = "untested".into();
        self.active_check = None;
        true
    }
}
pub fn credential(reference: &str) -> Result<keyring::Entry> {
    let service = if reference.starts_with("m0-") {
        "Learnloom.M0.Probe"
    } else if reference.starts_with("learnloom-") {
        "Learnloom.ModelConfig"
    } else {
        return Err("INVALID_CREDENTIAL_REFERENCE".into());
    };
    keyring::Entry::new(service, reference).map_err(|e| e.to_string())
}
pub fn write_credential(reference: &str, value: &str) -> Result<()> {
    credential(reference)?
        .set_password(value)
        .map_err(|e| e.to_string())
}
pub fn read_credential(reference: &str) -> Result<String> {
    credential(reference)?
        .get_password()
        .map_err(|_| "CREDENTIAL_UNAVAILABLE".into())
}

/// User-facing cloud settings boundary. Test mock snapshots bypass this entry point.
pub fn validate_cloud(endpoint: &str, model: &str) -> Result<String> {
    let url = reqwest::Url::parse(endpoint.trim()).map_err(|_| "INVALID_CLOUD_URL")?;
    let host = url
        .host_str()
        .ok_or("INVALID_CLOUD_URL")?
        .trim_matches(['[', ']'])
        .to_lowercase();
    let local_ip = host
        .parse::<std::net::IpAddr>()
        .map(|ip| match ip {
            std::net::IpAddr::V4(ip) => {
                ip.is_private()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_broadcast()
            }
            std::net::IpAddr::V6(ip) => {
                ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local()
                    || ip.is_multicast()
                    || ip.to_ipv4_mapped().is_some()
            }
        })
        .unwrap_or(false);
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || host == "localhost"
        || host.ends_with(".localhost")
        || host.ends_with(".local")
        || !host.contains('.')
        || local_ip
        || model.trim().is_empty()
    {
        return Err("CLOUD_HTTPS_REQUIRED: 请填写 HTTPS 云端地址和模型 ID".into());
    }
    Ok(url.as_str().trim_end_matches('/').to_string())
}
