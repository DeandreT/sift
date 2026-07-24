//! Parsing of the ATOM envelopes returned by the management API.
//!
//! The service emits XML namespaces inconsistently (`d2p1:` prefixes, default
//! namespaces, `i:nil` markers), so parsing matches on *local* element names
//! with a streaming reader instead of deriving serde types.

use quick_xml::Reader;
use quick_xml::events::Event;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::error::MgmtError;
use crate::model::NamespaceInfo;

/// Parse the `<entry>` returned by `GET /$namespaceinfo`.
pub(crate) fn parse_namespace_info(xml: &str) -> Result<NamespaceInfo, MgmtError> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut info = NamespaceInfo::default();
    let mut current: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                current = if is_nil(&e) {
                    None
                } else {
                    Some(String::from_utf8_lossy(e.local_name().as_ref()).into_owned())
                };
            }
            Ok(Event::End(_)) => current = None,
            Ok(Event::Text(e)) => {
                let text = e
                    .decode()
                    .map_err(|err| MgmtError::Xml(err.to_string()))?
                    .into_owned();
                if let Some(element) = current.as_deref() {
                    apply_field(&mut info, element, text.trim());
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(e) => return Err(MgmtError::Xml(e.to_string())),
        }
    }

    if info.name.is_empty() {
        return Err(MgmtError::Xml(
            "response did not contain a NamespaceInfo Name element".into(),
        ));
    }
    Ok(info)
}

/// True when the element carries `i:nil="true"`.
fn is_nil(e: &quick_xml::events::BytesStart<'_>) -> bool {
    e.attributes()
        .flatten()
        .any(|attr| attr.key.local_name().as_ref() == b"nil" && attr.value.as_ref() == b"true")
}

fn apply_field(info: &mut NamespaceInfo, element: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    match element {
        "Name" => text.clone_into(&mut info.name),
        "Alias" => info.alias = Some(text.to_owned()),
        "NamespaceType" => info.namespace_type = Some(text.to_owned()),
        "MessagingSKU" => info.messaging_sku = Some(text.to_owned()),
        "MessagingUnits" => info.messaging_units = text.parse().ok(),
        "CreatedTime" => info.created_time = parse_time(text),
        "ModifiedTime" => info.modified_time = parse_time(text),
        _ => {}
    }
}

fn parse_time(text: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(text, &Rfc3339).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured shape of a real `$namespaceinfo` response.
    const NAMESPACE_INFO: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<entry xmlns="http://www.w3.org/2005/Atom">
  <id>https://contoso.servicebus.windows.net/$namespaceinfo?api-version=2021-05</id>
  <title type="text">contoso</title>
  <updated>2026-07-20T10:00:00Z</updated>
  <author><name>contoso</name></author>
  <link rel="self" href="https://contoso.servicebus.windows.net/$namespaceinfo?api-version=2021-05"/>
  <content type="application/xml">
    <NamespaceInfo xmlns="http://schemas.microsoft.com/netservices/2010/10/servicebus/connect"
                   xmlns:i="http://www.w3.org/2001/XMLSchema-instance">
      <Alias i:nil="true"/>
      <CreatedTime>2024-03-01T08:30:00.123Z</CreatedTime>
      <MessagingSKU>Standard</MessagingSKU>
      <MessagingUnits i:nil="true">0</MessagingUnits>
      <ModifiedTime>2026-07-20T10:00:00Z</ModifiedTime>
      <Name>contoso</Name>
      <NamespaceType>Messaging</NamespaceType>
    </NamespaceInfo>
  </content>
</entry>"#;

    #[test]
    fn parses_namespace_info_entry() {
        let info = parse_namespace_info(NAMESPACE_INFO).unwrap();
        assert_eq!(info.name, "contoso");
        assert_eq!(info.namespace_type.as_deref(), Some("Messaging"));
        assert_eq!(info.messaging_sku.as_deref(), Some("Standard"));
        assert!(info.alias.is_none());
        // nil-marked elements are ignored even when they contain text
        assert!(info.messaging_units.is_none());
        assert_eq!(info.created_time.unwrap().year(), 2024);
        assert_eq!(info.modified_time.unwrap().year(), 2026);
    }

    #[test]
    fn missing_name_is_an_error() {
        let err = parse_namespace_info("<entry></entry>").unwrap_err();
        assert!(matches!(err, MgmtError::Xml(_)));
    }
}
