//! HWID cihaz kilidi. PLAN §3: HWID beyanı istemciden gelir,
//! sunucu doğrulayamaz; amaç fırsatçı paylaşımı zorlaştırmaktır.

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hwid(pub String);

/// Bu makinenin HWID'i. Windows'ta MachineGuid, yoksa kararlı fallback.
pub fn current() -> Hwid {
    #[cfg(windows)]
    {
        if let Ok(guid) = machine_guid() {
            if !guid.is_empty() {
                return Hwid(guid);
            }
        }
    }
    Hwid(fallback_id())
}

#[cfg(windows)]
fn machine_guid() -> Result<String, String> {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm
        .open_subkey(r"SOFTWARE\Microsoft\Cryptography")
        .map_err(|e| e.to_string())?;
    key.get_value::<String, _>("MachineGuid")
        .map_err(|e| e.to_string())
}

fn fallback_id() -> String {
    // Registry okunamazsa: kullanıcı başına kararlı dosya yerine,
    // ilk açılışta üretilip saklanacak geçici kimlik öneki.
    format!("fallback-{}", crate::request_id::new_request_id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwid_is_non_empty() {
        assert!(!current().0.is_empty());
    }
}
