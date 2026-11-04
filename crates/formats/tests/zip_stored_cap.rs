use craft_anything_formats::zip::{self, ZipError};

#[test]
fn a_stored_entry_cannot_understate_its_size_to_bypass_the_output_limit() {
    let data = craft_anything_testkit::electron::zip_stored(&[("payload", &[0x5a; 64])]);
    let mut entry = zip::parse(&data, 10).unwrap().entries.remove(0);
    entry.uncompressed_size = 1;
    assert!(matches!(zip::read_entry(&data, &entry, 8), Err(ZipError::TooLarge(8))));
    assert!(matches!(zip::read_entry_unverified(&data, &entry, 8), Err(ZipError::TooLarge(8))));
}

#[test]
fn stored_entries_at_the_limit_and_empty_entries_still_read() {
    let data = craft_anything_testkit::electron::zip_stored(&[("payload", b"hello"), ("empty", b"")]);
    let entries = zip::parse(&data, 10).unwrap().entries;
    assert_eq!(zip::read_entry(&data, &entries[0], 5).unwrap(), b"hello");
    assert_eq!(zip::read_entry(&data, &entries[1], 0).unwrap(), b"");
}
