use modlock_helper::catalog::provider::{
    Provider, ProviderDocument, ProviderRead, SubmissionKeyV1,
};
use json::{object, JsonValue};

fn gallery() -> JsonValue {
    json::parse(include_str!("fixtures/catalog/large-gallery.json")).unwrap()
}
fn index(row: JsonValue) -> Vec<u8> {
    object! {_aMetadata:object!{_nRecordCount:1,_bIsComplete:true,_nPerpage:50},_aRecords:vec![row]}
        .dump()
        .into_bytes()
}
fn read() -> ProviderRead {
    ProviderRead::Index {
        submission_type: "mod".into(),
        page: 1,
        latest: true,
    }
}

#[test]
fn gallery_over_presentation_limit_keeps_submission_and_first_32_images() {
    let row = gallery();
    assert_eq!(row["_aPreviewMedia"]["_aImages"].len(), 39);
    let ProviderDocument::Page { records, .. } =
        Provider::normalize(read(), &index(row.clone())).unwrap()
    else {
        panic!("index page")
    };
    assert_eq!(records[0].key.submission_id, "900000001");
    assert_eq!(records[0].data["images"].len(), 32);
    assert_eq!(
        records[0].data["images"][31],
        "https://example.invalid/modlock/gallery/panel-32.jpg"
    );
    assert_eq!(
        records[0].data["thumbnailUrl"],
        "https://example.invalid/modlock/gallery/panel-01-thumbnail.jpg"
    );
    let key = SubmissionKeyV1::from_slug("900000001").unwrap();
    let ProviderDocument::Profile(record) =
        Provider::normalize(ProviderRead::Profile(key), row.dump().as_bytes()).unwrap()
    else {
        panic!("profile")
    };
    assert_eq!(record.data["images"].len(), 32);
    let mut collection = row;
    collection["_sModelName"] = "Collection".into();
    collection["_sProfileUrl"] = "https://gamebanana.com/collections/900000001".into();
    let ProviderDocument::Collections { records, .. } =
        Provider::normalize(ProviderRead::Collections { page: 1 }, &index(collection)).unwrap()
    else {
        panic!("collections")
    };
    assert_eq!(records[0]["images"].len(), 32);
}

#[test]
fn gallery_truncation_still_rejects_mismatched_identity_and_malformed_tail() {
    for (field, value) in [
        ("_idRow", 1.into()),
        ("_sModelName", "Sound".into()),
        ("_sProfileUrl", "https://gamebanana.com/mods/1".into()),
        ("_aGame", object! {_idRow:1}),
    ] {
        let mut row = gallery();
        row[field] = value;
        let key = SubmissionKeyV1::from_slug("900000001").unwrap();
        assert!(
            Provider::normalize(ProviderRead::Profile(key), row.dump().as_bytes()).is_err(),
            "{field}"
        );
    }
    for value in [
        JsonValue::Null,
        "../unsafe.jpg".into(),
        "image.jpg?query".into(),
        42.into(),
    ] {
        let mut row = gallery();
        row["_aPreviewMedia"]["_aImages"][38]["_sFile"] = value;
        assert!(Provider::normalize(read(), &index(row)).is_err());
    }
}

#[test]
fn empty_collection_negative_one_count_is_zero_but_other_bad_counts_fail() {
    // An empty Collection Index row can report -1. Only this sentinel is accepted.
    let row = json::parse(include_str!("fixtures/catalog/empty-collection.json")).unwrap();
    let ProviderDocument::Collections { records, .. } =
        Provider::normalize(ProviderRead::Collections { page: 4 }, &index(row.clone())).unwrap()
    else {
        panic!("collections")
    };
    assert_eq!(records[0]["itemCount"], "0");
    assert_eq!(records[0]["collectionId"], "900000003");
    for bad in [(-2).into(), "-1".into(), 1.5.into(), true.into()] {
        let mut row = row.clone();
        row["_nItemCount"] = bad;
        assert!(Provider::normalize(ProviderRead::Collections { page: 4 }, &index(row)).is_err());
    }
}
