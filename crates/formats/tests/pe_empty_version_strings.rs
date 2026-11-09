use craft_anything_formats::pe;
use std::collections::BTreeMap;

/// One version-resource node: the header, the key, then the value and children, each 4-aligned.
fn node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    for part in std::iter::once(value).chain(children.iter().map(Vec::as_slice)) {
        while !b.len().is_multiple_of(4) {
            b.push(0);
        }
        b.extend_from_slice(part);
    }
    let len = b.len() as u16;
    b[0..2].copy_from_slice(&len.to_le_bytes());
    b[2..4].copy_from_slice(&value_len.to_le_bytes());
    b[4..6].copy_from_slice(&u16::from(text).to_le_bytes());
    b
}

fn string(key: &str, value: &str) -> Vec<u8> {
    let units: Vec<u8> = value.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    node(key, &units, (units.len() / 2) as u16, true, &[])
}

/// What Windows resource compilers write for an empty value: no value at all (wValueLength 0).
fn empty_string(key: &str) -> Vec<u8> {
    node(key, &[], 0, true, &[])
}

fn version_info(tables: &[Vec<u8>]) -> Vec<u8> {
    let mut fixed = vec![0u8; 52];
    fixed[0..4].copy_from_slice(&0xfeef_04bd_u32.to_le_bytes());
    let file_info = node("StringFileInfo", &[], 0, true, tables);
    node("VS_VERSION_INFO", &fixed, 52, false, &[file_info])
}

/// Keys with an empty value are kept. `OleSelfRegister` only means something by being there.
#[test]
fn version_strings_with_empty_values_are_kept() {
    let table = node("040904b0", &[], 0, true, &[string("CompanyName", "Craft"), empty_string("OleSelfRegister"), empty_string("InternalName")]);
    let info = pe::parse_version_info(&version_info(&[table])).unwrap();
    let expected: BTreeMap<String, String> =
        [("CompanyName", "Craft"), ("InternalName", ""), ("OleSelfRegister", "")].into_iter().map(|(k, v)| (k.into(), v.into())).collect();
    assert_eq!(info.strings, expected);
}

/// A string table with no strings is a table, not an empty string named after its language.
#[test]
fn an_empty_string_table_is_not_a_string() {
    let empty_table = node("000004b0", &[], 0, true, &[]);
    let table = node("040904b0", &[], 0, true, &[string("ProductName", "Craft")]);
    let info = pe::parse_version_info(&version_info(&[empty_table, table])).unwrap();
    assert_eq!(info.strings.keys().collect::<Vec<_>>(), ["ProductName"]);
}
