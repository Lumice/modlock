use modlock_helper::catalog::provider::{
    Provider, ProviderDocument, ProviderRead, SubmissionKeyV1,
};
#[test]
fn profile_exposes_rich_description_and_preserves_plain_and_links() {
    let mut row = json::parse(include_str!("fixtures/catalog/large-gallery.json")).unwrap();
    row["_sText"] =
        "<h2>Heading</h2><b>Bold</b> <a href='https://example.invalid/modlock/details'>Link</a>".into();
    let ProviderDocument::Profile(record) = Provider::normalize(
        ProviderRead::Profile(SubmissionKeyV1::from_slug("900000001").unwrap()),
        row.dump().as_bytes(),
    )
    .unwrap() else {
        panic!("profile")
    };
    let detail = record.detail.as_ref().unwrap();
    assert_eq!(detail["richDescription"][0]["type"], "heading");
    assert_eq!(detail["richDescription"][1]["runs"][0]["bold"], true);
    assert_eq!(detail["description"], "Heading\n\nBold Link");
    assert_eq!(
        detail["links"][0]["url"],
        "https://example.invalid/modlock/details"
    );

    use modlock_helper::catalog::{details::ResourceQuery, store::CatalogStore};
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!(
        "catalog-rich-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("cache/catalog")).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut store = CatalogStore::open(&root).unwrap();
    store.publish_profile(record).unwrap();
    let query = ResourceQuery {
        kind: "detail".into(),
        key: SubmissionKeyV1::from_slug("900000001").unwrap(),
        page: 1,
    };
    let cached = store.read_resource(&query).unwrap();
    assert_eq!(
        cached.data["resource"]["richDescription"][0]["type"],
        "heading"
    );
    let mut legacy = cached.data["resource"].clone();
    legacy.remove("richDescription");
    store
        .put_resource("detail", "900000001", &legacy, true)
        .unwrap();
    assert_eq!(
        store.read_resource(&query).unwrap().data["resource"]["richDescription"].len(),
        0
    );
    drop(store);
    std::fs::remove_dir_all(root).unwrap();
}
