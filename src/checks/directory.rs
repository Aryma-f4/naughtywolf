//! Bounded, read-only directory enumeration for one approved asset.
//!
//! This check observes structure and posture only: it lists directory objects
//! and reports the attributes that indicate a domain is *reachable* by
//! credential-theft tooling. It never reads, requests, stores, or transmits
//! credential material — no `unicodePwd`, `unicodePwd` history, `ntlm hashes`,
//! Kerberos ticket material, or DRSync-style writes are ever requested.
use super::{CheckFinding, CheckResult};
use ldap3::{LdapConnAsync, Scope, SearchEntry, drive};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Directory objects this check knows how to summarize.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ObjectClass {
    #[default]
    User,
    Group,
    Computer,
    All,
}

impl ObjectClass {
    fn filter(self) -> &'static str {
        match self {
            Self::User => "(objectCategory=person)(objectClass=user)",
            Self::Group => "(objectCategory=group)",
            Self::Computer => "(objectCategory=computer)",
            Self::All => {
                "(|(objectCategory=person)(objectCategory=group)(objectCategory=computer))"
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectoryEnumInput {
    /// Base DN to search, e.g. `DC=corp,DC=example`. Empty uses the naming context.
    #[serde(default)]
    pub base_dn: String,
    /// Simple-bind DN. Empty performs an anonymous bind.
    #[serde(default)]
    pub bind_dn: String,
    /// Simple-bind password. Redacted before the run is persisted.
    #[serde(default)]
    pub bind_password: String,
    #[serde(default)]
    pub object_class: ObjectClass,
    /// Hard ceiling on entries returned per search. Clamped to `MAX_ENTRIES`.
    #[serde(default)]
    pub limit: usize,
}

/// Attributes requested for every entry. Deliberately excludes anything that
/// can carry a credential so a permissive directory cannot push secrets into a
/// check result that gets stored and rendered.
const USER_ATTRIBUTES: &[&str] = &[
    "samaccountname",
    "cn",
    "useraccountcontrol",
    "pwdlastset",
    "lastlogon",
    "memberof",
    "serviceprincipalname",
    "msds-keycredentiallink",
    "msds-allowedtodelegateto",
    "msds-allowedtoactonbehalfofotheridentity",
    "usercertificate",
];
const GROUP_ATTRIBUTES: &[&str] = &["cn", "member", "memberof", "description"];
const COMPUTER_ATTRIBUTES: &[&str] = &[
    "cn",
    "samaccountname",
    "operatingsystem",
    "operatingsystemversion",
    "dnshostname",
    "serviceprincipalname",
    "trustedfordelegation",
    "msds-allowedtodelegateto",
    "msds-allowedtoactonbehalfofotheridentity",
];

const MAX_ENTRIES: usize = 200;
const MAX_FINDINGS: usize = 200;

/// `userAccountControl` bits that matter for credential-theft reachability.
const UF_ACCOUNTDISABLE: u32 = 0x0002;
const UF_DONT_REQUIRE_PREAUTH: u32 = 0x0040;
const UF_TRUSTED_FOR_DELEGATION: u32 = 0x0800_0000;

fn finding(code: &str, message: impl Into<String>) -> CheckFinding {
    CheckFinding {
        code: code.to_owned(),
        message: message.into().chars().take(512).collect(),
    }
}

/// Rejects any attribute name that could carry credential material, so the
/// constant lists above fail loudly if they are ever extended carelessly.
pub fn is_safe_attribute(attribute: &str) -> bool {
    // Arms are lowercase because the input is lowercased first; LDAP attribute
    // names are case-insensitive and are written in every conceivable casing.
    !matches!(
        attribute.to_ascii_lowercase().as_str(),
        "unicodepwd"
            | "ntlmpwdhistory"
            | "dbcspwd"
            | "lmpwdhistory"
            | "supplementalcredentials"
            | "ms-mcs-admispwrd"
            | "unicodepwdhistory"
    )
}

/// Case-insensitive single-value attribute lookup. Directories echo back
/// attribute names inconsistently, so never rely on exact casing.
fn lookup<'a>(attrs: &'a HashMap<String, Vec<String>>, name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
}

fn lookup_all<'a>(attrs: &'a HashMap<String, Vec<String>>, name: &str) -> Vec<&'a str> {
    attrs
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, values)| values.iter().map(String::as_str).collect())
        .unwrap_or_default()
}

fn present(value: Option<&str>) -> bool {
    value
        .map(|raw| !raw.trim().is_empty() && !raw.trim().eq_ignore_ascii_case("false"))
        .unwrap_or(false)
}

/// Posture signals that indicate *why* credential theft would succeed here.
/// These are the same conditions BloodHound surfaces, reported without
/// requesting a single credential.
fn posture_findings(
    prefix: &str,
    label: &str,
    attrs: &HashMap<String, Vec<String>>,
    findings: &mut Vec<CheckFinding>,
) {
    let account_control =
        lookup(attrs, "userAccountControl").and_then(|raw| raw.trim().parse::<u32>().ok());

    if let Some(control) = account_control {
        if control & UF_DONT_REQUIRE_PREAUTH != 0 {
            findings.push(finding(
                "posture.asrep_roastable",
                format!(
                    "{prefix} {label}: userAccountControl 0x{:08x} has DONT_REQUIRE_PREAUTH; \
                     this account is AS-REP roastable.",
                    control
                ),
            ));
        }
        if control & UF_TRUSTED_FOR_DELEGATION != 0 {
            findings.push(finding(
                "posture.unconstrained_delegation",
                format!(
                    "{prefix} {label}: TRUSTED_FOR_DELEGATION is set; unconstrained \
                     delegation exposes this host's credentials to a rogue DC.",
                    label = label
                ),
            ));
        }
    }

    if present(lookup(attrs, "msDS-KeyCredentialLink")) {
        findings.push(finding(
            "posture.key_credential_link",
            format!(
                "{prefix} {label}: msDS-KeyCredentialLink is present. Passwords can be \
                 recovered from credential history; this is the primary route Mimikatz-class \
                 tooling uses to obtain current password material."
            ),
        ));
    }

    if present(lookup(attrs, "msDS-AllowedToDelegateTo"))
        || present(lookup(attrs, "msDS-AllowedToActOnBehalfOfOtherIdentity"))
    {
        findings.push(finding(
            "posture.constrained_delegation",
            format!(
                "{prefix} {label}: delegation is configured. Combined with a delegated \
                 credential or a rogue DC, this permits impersonation to the named service."
            ),
        ));
    }

    if account_control.is_some_and(|control| control & UF_ACCOUNTDISABLE != 0) {
        findings.push(finding(
            "posture.disabled_not_removed",
            format!(
                "{prefix} {label}: account is disabled but still present. Disabled accounts \
                 remain a takeover target when re-enabled."
            ),
        ));
    }
}

/// Resolves the LDAP endpoint for an approved asset address, defaulting to the
/// standard LDAP port and preserving an explicit `ldaps://` or `ldap://` scheme.
pub fn endpoint(address: &str, secure: bool) -> Result<String, &'static str> {
    // Validate the raw input, not the trimmed value: trimming first would let
    // trailing control characters through.
    if address.trim().is_empty() || address.len() > 2048 || address.chars().any(char::is_control) {
        return Err("Asset address is empty or invalid.");
    }
    let value = address.trim();
    let scheme = if secure { "ldaps" } else { "ldap" };
    let authority = value
        .split("://")
        .last()
        .unwrap_or(value)
        .split('/')
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() || authority.contains('@') || authority.contains(char::is_whitespace) {
        return Err("Use a hostname or IP address without embedded credentials.");
    }
    if authority.contains(':') && !authority.ends_with(":636") && !authority.ends_with(":389") {
        return Err("Only the standard LDAP ports 389 and 636 are supported.");
    }
    Ok(format!("{scheme}://{authority}"))
}

pub async fn enumerate(
    address: &str,
    input: DirectoryEnumInput,
) -> Result<CheckResult, &'static str> {
    // A password implies `ldaps`; without one we stay on plain `ldap` so the
    // observer never chooses a downgrade silently. LDAP Simple Bind sends the
    // password in the clear, so that case must be deliberate.
    let url = endpoint(address, !input.bind_password.is_empty())?;
    let object_class = input.object_class;
    let limit = input.limit.clamp(1, MAX_ENTRIES);

    let mut attributes = match object_class {
        ObjectClass::Group => GROUP_ATTRIBUTES.to_vec(),
        ObjectClass::Computer => COMPUTER_ATTRIBUTES.to_vec(),
        ObjectClass::User | ObjectClass::All => {
            let mut all = USER_ATTRIBUTES.to_vec();
            all.extend_from_slice(GROUP_ATTRIBUTES);
            all.extend_from_slice(COMPUTER_ATTRIBUTES);
            all
        }
    };
    attributes.push("objectclass");

    // Fail closed rather than transmit anything unvetted.
    if let Some(unsafe_attribute) = attributes.iter().find(|name| !is_safe_attribute(name)) {
        return Ok(CheckResult::failed(format!(
            "Refusing to request sensitive attribute {unsafe_attribute}."
        )));
    }

    let parsed = reqwest::Url::parse(&url).map_err(|_| "Invalid LDAP endpoint.")?;
    let (connection, mut ldap) = LdapConnAsync::from_url(&parsed)
        .await
        .map_err(|_| "Could not open an LDAP connection to the asset.")?;
    drive!(connection);

    if let Err(error) = ldap
        .simple_bind(&input.bind_dn, &input.bind_password)
        .await
        .map_err(|_| "LDAP bind failed.")?
        .success()
    {
        return Ok(CheckResult::failed(format!(
            "LDAP bind rejected ({error}). Verify the base DN and bind credentials."
        )));
    }

    let mut findings = vec![
        finding("directory.endpoint", url),
        finding(
            "directory.object_class",
            format!("{object_class:?}").to_lowercase(),
        ),
    ];
    if input.bind_dn.trim().is_empty() {
        findings.push(finding(
            "directory.bind",
            "Anonymous bind accepted. Anonymous read access to the directory is itself a finding.",
        ));
    }

    let search_base = if input.base_dn.trim().is_empty() {
        ldap.search(
            "",
            Scope::Base,
            "(objectClass=*)",
            vec!["defaultNamingContext"],
        )
        .await
        .map_err(|_| "Could not read the rootDSE.")?
        .success()
        .ok()
        .and_then(|(entries, _)| entries.into_iter().next())
        .and_then(|entry| {
            let entry = SearchEntry::construct(entry);
            lookup(&entry.attrs, "defaultNamingContext").map(str::to_owned)
        })
    } else {
        Some(input.base_dn.trim().to_owned())
    };
    let Some(search_base) = search_base else {
        return Ok(CheckResult::failed(
            "Could not determine the naming context; provide a base DN.",
        ));
    };
    findings.push(finding("directory.base_dn", search_base.clone()));

    let entries = match ldap
        .search(
            &search_base,
            Scope::Subtree,
            object_class.filter(),
            &attributes,
        )
        .await
        .map_err(|_| "LDAP search failed.")?
        .success()
    {
        Ok((entries, _)) => entries
            .into_iter()
            .map(SearchEntry::construct)
            .take(limit)
            .collect::<Vec<SearchEntry>>(),
        Err(error) => {
            return Ok(CheckResult::failed(format!(
                "LDAP search failed ({error}). Check the base DN and bind credentials."
            )));
        }
    };

    if entries.len() >= limit {
        findings.push(finding(
            "directory.truncated",
            format!("Showing the first {limit} entries. Raise the limit or narrow the base DN."),
        ));
    }

    let mut users = 0usize;
    let mut groups = 0usize;
    let mut computers = 0usize;
    for entry in &entries {
        let classes = lookup_all(&entry.attrs, "objectclass");
        let label = lookup(&entry.attrs, "cn")
            .or_else(|| lookup(&entry.attrs, "samaccountname"))
            .map(str::to_owned)
            .unwrap_or_else(|| entry.dn.clone());

        if classes.iter().any(|class| {
            matches!(
                class.to_ascii_lowercase().as_str(),
                "user" | "person" | "organizationalperson"
            )
        }) {
            users += 1;
            if findings.len() < MAX_FINDINGS {
                findings.push(finding("directory.user", label.clone()));
            }
            posture_findings("User", &label, &entry.attrs, &mut findings);
        } else if classes
            .iter()
            .any(|class| class.eq_ignore_ascii_case("group"))
        {
            groups += 1;
            if findings.len() < MAX_FINDINGS {
                findings.push(finding("directory.group", label.clone()));
            }
        } else {
            computers += 1;
            if findings.len() < MAX_FINDINGS {
                findings.push(finding("directory.computer", label.clone()));
            }
            posture_findings("Computer", &label, &entry.attrs, &mut findings);
        }
        findings.truncate(MAX_FINDINGS);
    }

    findings.push(finding("directory.count.users", users.to_string()));
    findings.push(finding("directory.count.groups", groups.to_string()));
    findings.push(finding("directory.count.computers", computers.to_string()));
    if users == 0 && groups == 0 && computers == 0 {
        findings.push(finding(
            "directory.empty",
            "No directory objects were returned. Confirm the base DN and bind credentials.",
        ));
    }
    findings.push(finding(
        "directory.credentials_not_collected",
        "This check never requests passwords, hashes, or ticket material. Findings describe posture only.",
    ));

    ldap.unbind().await.ok();
    Ok(CheckResult::succeeded(findings))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checks::RunState;

    #[test]
    fn refuses_credential_bearing_attributes() {
        assert!(!is_safe_attribute("unicodePwd"));
        assert!(!is_safe_attribute("NTLMPwdHistory"));
        assert!(!is_safe_attribute("dBCSPwd"));
        assert!(!is_safe_attribute("SupplementalCredentials"));
        assert!(is_safe_attribute("samaccountname"));
        assert!(is_safe_attribute("msDS-AllowedToDelegateTo"));
    }

    #[test]
    fn catalog_attribute_lists_are_safe_to_send() {
        for attribute in USER_ATTRIBUTES
            .iter()
            .chain(GROUP_ATTRIBUTES)
            .chain(COMPUTER_ATTRIBUTES)
        {
            assert!(is_safe_attribute(attribute), "{attribute}");
        }
    }

    #[test]
    fn endpoint_uses_the_standard_ports_and_rejects_credentials() {
        assert_eq!(
            endpoint("dc1.corp.test", false).unwrap(),
            "ldap://dc1.corp.test"
        );
        assert_eq!(
            endpoint("ldaps://dc1.corp.test", true).unwrap(),
            "ldaps://dc1.corp.test"
        );
        assert_eq!(
            endpoint("10.0.0.5:389", false).unwrap(),
            "ldap://10.0.0.5:389"
        );
        for value in ["", "ldap://user:secret@dc1", "dc1:8080", "dc1\n"] {
            assert!(endpoint(value, false).is_err(), "{value}");
        }
    }

    fn attributes(pairs: &[(&str, &str)]) -> HashMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), vec![(*value).to_owned()]))
            .collect()
    }

    #[test]
    fn flags_as_rep_roastable_and_key_credential_link() {
        let mut findings = Vec::new();
        // 0x0040 = DONT_REQUIRE_PREAUTH, no ACCOUNTDISABLE bit.
        posture_findings(
            "User",
            "svc-web",
            &attributes(&[
                ("userAccountControl", "64"),
                ("msDS-KeyCredentialLink", "[objectSid]\"-RDU\""),
            ]),
            &mut findings,
        );
        let codes: Vec<_> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"posture.asrep_roastable"));
        assert!(codes.contains(&"posture.key_credential_link"));
        assert!(
            findings.iter().all(|f| !f.message.contains("\"-RDU\"")),
            "raw key-credential-link blob must not be echoed"
        );
    }

    #[test]
    fn flags_unconstrained_delegation_and_delegation_configuration() {
        let mut findings = Vec::new();
        posture_findings(
            "Computer",
            "HOST01",
            &attributes(&[
                ("userAccountControl", "134217728"),
                ("msDS-AllowedToDelegateTo", "MSSQLSvc/dc2.corp.test"),
            ]),
            &mut findings,
        );
        let codes: Vec<_> = findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"posture.unconstrained_delegation"));
        assert!(codes.contains(&"posture.constrained_delegation"));
    }

    #[test]
    fn flags_disabled_account_that_survives() {
        let mut findings = Vec::new();
        posture_findings(
            "User",
            "old-admin",
            &attributes(&[("userAccountControl", "514")]),
            &mut findings,
        );
        assert!(
            findings
                .iter()
                .any(|f| f.code == "posture.disabled_not_removed")
        );
    }

    #[test]
    fn a_clean_account_produces_no_posture_findings() {
        let mut findings = Vec::new();
        posture_findings(
            "User",
            "jdoe",
            &attributes(&[("userAccountControl", "512"), ("cn", "jdoe")]),
            &mut findings,
        );
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[tokio::test]
    async fn a_peer_that_is_not_an_ldap_server_fails_without_panicking() {
        // Deterministic and instant: a listener that accepts then drops is the
        // same shape as a port-forward to something that is not a DC.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
        });
        let result = enumerate(
            &address.to_string(),
            DirectoryEnumInput {
                base_dn: "DC=corp,DC=example".to_owned(),
                bind_dn: String::new(),
                bind_password: String::new(),
                object_class: ObjectClass::User,
                limit: 10,
            },
        )
        .await;
        // Either a transport error or a well-formed failure result is fine; the
        // point is that it never panics and never reports a bogus success.
        if let Ok(CheckResult { state, .. }) = result {
            assert_ne!(state, RunState::Succeeded);
        }
    }
}
