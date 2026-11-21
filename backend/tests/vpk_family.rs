mod support;
use modlock_helper::vpk::{
    inspect, inspect_directory, read_resource, required_archives, validate_family,
};
use std::{collections::BTreeMap, fs, os::unix::fs::symlink};
use support::{family_vpk, vpk_with_tree, FamilyEntry, Fixture};

fn entry<'a>(
    path: &'a str,
    archive: u16,
    offset: u32,
    length: u32,
    preload: &'a [u8],
) -> FamilyEntry<'a> {
    FamilyEntry {
        path,
        crc: 42,
        archive,
        offset,
        length,
        preload,
    }
}

#[test]
fn family_v1_v2_inline_zero_sparse_and_max_index() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let mut companions = BTreeMap::new();
    for (index, bytes) in [(0, b"!zero?".as_slice()), (17, b"sparse"), (65535, b"max")] {
        let p = f.root.join(format!("member-{index}"));
        fs::write(&p, bytes).unwrap();
        companions.insert(index, p);
    }
    let entries = [
        entry("Models/A.bin", 0, 1, 4, b"pre-"),
        entry("b.bin", 17, 0, 6, b""),
        entry("c.bin", 65535, 0, 3, b""),
        entry("d.bin", 0x7fff, 2, 6, b"in-"),
    ];
    for version in [1, 2] {
        let bytes = family_vpk(version, &entries, b"xxinline!");
        fs::write(&path, &bytes).unwrap();
        let directory = inspect_directory(&path).unwrap();
        assert_eq!(
            required_archives(&directory)
                .into_iter()
                .collect::<Vec<_>>(),
            [0, 17, 65535]
        );
        assert_eq!(directory.inline_offset, (bytes.len() - 9) as u64);
        assert_eq!(directory.inline_size, 9);
        assert_eq!(directory.resources[0].entry.path, "models/a.bin");
        assert_eq!(directory.resources[0].entry.crc, 42);
        assert_eq!(directory.resources[0].location.preload, b"pre-");
        validate_family(&directory, &companions).unwrap();
        for (i, expected) in [b"pre-zero".as_slice(), b"sparse", b"max", b"in-inline"]
            .iter()
            .enumerate()
        {
            assert_eq!(
                read_resource(&path, &directory, &companions, i, 1024).unwrap(),
                *expected
            );
        }
        assert!(
            inspect(&path).is_err(),
            "standalone wrapper must reject split files"
        );
    }
}

#[test]
fn shared_index_is_one_dependency() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let member = f.root.join("opaque");
    fs::write(&member, b"abcdef").unwrap();
    fs::write(
        &path,
        family_vpk(
            1,
            &[entry("a", 17, 0, 3, b""), entry("b", 17, 3, 3, b"p")],
            b"",
        ),
    )
    .unwrap();
    let directory = inspect_directory(&path).unwrap();
    assert_eq!(
        required_archives(&directory)
            .into_iter()
            .collect::<Vec<_>>(),
        [17]
    );
    let companions = BTreeMap::from([(17, member)]);
    validate_family(&directory, &companions).unwrap();
    assert_eq!(
        read_resource(&path, &directory, &companions, 0, 16).unwrap(),
        b"abc"
    );
    assert_eq!(
        read_resource(&path, &directory, &companions, 1, 16).unwrap(),
        b"pdef"
    );
}

#[test]
fn distinct_indices_can_share_owned_bytes() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let member = f.root.join("owned-immutable");
    fs::write(&member, b"abcdef").unwrap();
    fs::write(
        &path,
        family_vpk(
            2,
            &[entry("a", 0, 1, 2, b""), entry("b", 65535, 3, 3, b"!")],
            b"",
        ),
    )
    .unwrap();
    let directory = inspect_directory(&path).unwrap();
    let companions = BTreeMap::from([(0, member.clone()), (65535, member)]);
    validate_family(&directory, &companions).unwrap();
    assert_eq!(
        read_resource(&path, &directory, &companions, 0, 16).unwrap(),
        b"bc"
    );
    assert_eq!(
        read_resource(&path, &directory, &companions, 1, 16).unwrap(),
        b"!def"
    );
}

#[test]
fn preload_only_needs_no_companion() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let link = f.root.join("directory-link");
    symlink(&path, &link).unwrap();
    for version in [1, 2] {
        for archive in [0, 65535, 0x7fff] {
            for preload in [b"preload".as_slice(), b""] {
                fs::write(
                    &path,
                    family_vpk(version, &[entry("a", archive, u32::MAX, 0, preload)], b""),
                )
                .unwrap();
                let directory = inspect_directory(&path).unwrap();
                assert!(required_archives(&directory).is_empty());
                validate_family(&directory, &BTreeMap::new()).unwrap();
                assert_eq!(
                    read_resource(&path, &directory, &BTreeMap::new(), 0, preload.len()).unwrap(),
                    preload
                );
                if !preload.is_empty() {
                    assert!(read_resource(
                        &path,
                        &directory,
                        &BTreeMap::new(),
                        0,
                        preload.len() - 1
                    )
                    .is_err());
                }
                assert!(read_resource(&path, &directory, &BTreeMap::new(), 1, 16).is_err());
                assert!(read_resource(&link, &directory, &BTreeMap::new(), 0, 16).is_err());
                assert!(read_resource(&f.root, &directory, &BTreeMap::new(), 0, 16).is_err());
                assert!(read_resource(
                    &f.root.join("missing"),
                    &directory,
                    &BTreeMap::new(),
                    0,
                    16
                )
                .is_err());
            }
        }
    }
}

#[test]
fn strict_legacy_inspect_rejects_preload_only_external_indices() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    for version in [1, 2] {
        for archive in [0, 65535] {
            fs::write(
                &path,
                family_vpk(version, &[entry("a", archive, 0, 0, b"pre")], b""),
            )
            .unwrap();
            let directory = inspect_directory(&path).unwrap();
            assert!(required_archives(&directory).is_empty());
            assert_eq!(inspect(&path).unwrap_err().kind, "unsupportedVpk");
        }
    }
}

#[test]
fn preload_only_offsets_are_unused_in_mixed_families() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let member = f.root.join("member");
    fs::write(&member, b"abc").unwrap();
    let companions = BTreeMap::from([(0, member.clone()), (17, member)]);
    fs::write(
        &path,
        family_vpk(
            2,
            &[
                entry("a", 0, u32::MAX, 0, b"pre"),
                entry("b", 65535, u32::MAX, 0, b""),
                entry("c", 0x7fff, u32::MAX, 0, b"inline-pre"),
                entry("d", 0, 0, 3, b""),
                entry("e", 17, 0, 3, b""),
            ],
            b"",
        ),
    )
    .unwrap();
    let directory = inspect_directory(&path).unwrap();
    assert_eq!(
        required_archives(&directory)
            .into_iter()
            .collect::<Vec<_>>(),
        [0, 17]
    );
    validate_family(&directory, &companions).unwrap();
    for (index, expected) in [b"pre".as_slice(), b"", b"inline-pre", b"abc", b"abc"]
        .iter()
        .enumerate()
    {
        assert_eq!(
            read_resource(&path, &directory, &companions, index, 16).unwrap(),
            *expected
        );
    }
}

#[test]
fn family_rejects_out_of_range_and_truncation() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let member = f.root.join("member");
    fs::write(&member, b"abc").unwrap();
    let companions = BTreeMap::from([(17, member.clone())]);
    for (offset, length) in [(0, 4), (3, 1), (u32::MAX, u32::MAX)] {
        fs::write(
            &path,
            family_vpk(1, &[entry("a", 17, offset, length, b"")], b""),
        )
        .unwrap();
        let directory = inspect_directory(&path).unwrap();
        assert!(
            validate_family(&directory, &companions).is_err(),
            "external range {offset}+{length}"
        );
        assert!(read_resource(&path, &directory, &companions, 0, 16).is_err());
    }
    let inline = family_vpk(2, &[entry("a", 0x7fff, 1, 3, b"pre")], b"!abc");
    let with_sections = vpk_with_tree(
        2,
        &inline[28..inline.len() - 4],
        b"!abc",
        [b"archive-md5", b"other-md5", b"signature"],
    );
    fs::write(&path, &with_sections).unwrap();
    let section_directory = inspect_directory(&path).unwrap();
    assert_eq!(section_directory.inline_size, 4);
    assert_eq!(
        read_resource(&path, &section_directory, &BTreeMap::new(), 0, 6).unwrap(),
        b"preabc"
    );
    fs::write(&path, &inline).unwrap();
    let mut directory = inspect_directory(&path).unwrap();
    assert_eq!(
        read_resource(&path, &directory, &BTreeMap::new(), 0, 6).unwrap(),
        b"preabc"
    );
    directory.resources[0].location.offset = u64::MAX;
    directory.resources[0].location.length = 1;
    assert!(validate_family(&directory, &BTreeMap::new()).is_err());
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 16).is_err());
    directory = inspect_directory(&path).unwrap();
    directory.inline_offset = u64::MAX;
    assert!(validate_family(&directory, &BTreeMap::new()).is_err());
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 16).is_err());
    // Truncating after inspection must be caught at read time, before allocation.
    directory = inspect_directory(&path).unwrap();
    fs::write(&path, &inline[..inline.len() - 1]).unwrap();
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 16).is_err());
    // V2's declared inline section excludes its later sections.
    let b = family_vpk(1, &[entry("a", 0x7fff, 0, 2, b"")], b"");
    let tree = &b[12..];
    fs::write(&path, vpk_with_tree(2, tree, b"x", [b"y", b"z", b"!"])).unwrap();
    assert!(inspect_directory(&path).is_err());
    // Preload bytes and every tree terminator must fit inside the declared tree.
    let valid = family_vpk(1, &[entry("a", 0x7fff, 0, 0, b"PRELOAD")], b"");
    for bytes in [
        vpk_with_tree(1, &valid[12..valid.len() - 4], b"", [&[], &[], &[]]),
        {
            let mut b = valid.clone();
            b[12 + 4 + 2 + 4..12 + 4 + 2 + 6].copy_from_slice(&u16::MAX.to_le_bytes());
            b
        },
        {
            let mut b = valid.clone();
            let n = b.len();
            b[n - 12..n - 10].copy_from_slice(&0u16.to_le_bytes());
            b
        },
        {
            let mut b = inline.clone();
            b.push(0);
            b
        },
        {
            let mut b = inline.clone();
            b[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
            b
        },
    ] {
        fs::write(&path, bytes).unwrap();
        assert!(inspect_directory(&path).is_err());
    }
    fs::write(&path, family_vpk(1, &[entry("a", 17, 0, 3, b"")], b"")).unwrap();
    let directory = inspect_directory(&path).unwrap();
    fs::write(&member, b"ab").unwrap();
    assert!(validate_family(&directory, &companions).is_err());
    assert!(read_resource(&path, &directory, &companions, 0, 16).is_err());
}

#[test]
fn family_rejects_unsafe_duplicate_and_utf8_paths() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let boundary = "a".repeat(4096);
    fs::write(
        &path,
        family_vpk(1, &[entry(&boundary, 0x7fff, 0, 0, b"")], b""),
    )
    .unwrap();
    assert_eq!(
        inspect_directory(&path).unwrap().resources[0]
            .entry
            .path
            .len(),
        4096
    );
    let normalized_boundary = format!("{}.x", "a".repeat(4094));
    fs::write(
        &path,
        family_vpk(1, &[entry(&normalized_boundary, 0x7fff, 0, 0, b"")], b""),
    )
    .unwrap();
    assert_eq!(
        inspect_directory(&path).unwrap().resources[0]
            .entry
            .path
            .len(),
        4096
    );
    for unsafe_path in [
        "../a.x".to_owned(),
        "a/../b.x".to_owned(),
        "a\\..\\b/c.x".to_owned(),
        "a:bad.x".to_owned(),
        "a//b.x".to_owned(),
        "a".repeat(4097),
        format!("{}.x", "a".repeat(4095)),
    ] {
        fs::write(
            &path,
            family_vpk(1, &[entry(&unsafe_path, 0x7fff, 0, 0, b"")], b""),
        )
        .unwrap();
        assert!(inspect_directory(&path).is_err(), "{unsafe_path}");
    }
    fs::write(
        &path,
        family_vpk(
            1,
            &[
                entry("Models\\Nested/A.bin", 0x7fff, 0, 0, b""),
                entry("models/nested/a.BIN", 0x7fff, 0, 0, b""),
            ],
            b"",
        ),
    )
    .unwrap();
    assert!(inspect_directory(&path).is_err());
    let mut invalid_utf8 = family_vpk(1, &[entry("a.x", 0x7fff, 0, 0, b"")], b"");
    invalid_utf8[12] = 0xff;
    fs::write(&path, invalid_utf8).unwrap();
    assert!(inspect_directory(&path).is_err());
}

#[test]
fn family_rejects_missing_extra_and_symlink_members() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let member = f.root.join("member");
    fs::write(&path, family_vpk(1, &[entry("a", 17, 0, 1, b"")], b"")).unwrap();
    fs::write(&member, b"x").unwrap();
    let directory = inspect_directory(&path).unwrap();
    let good = BTreeMap::from([(17, member.clone())]);
    validate_family(&directory, &good).unwrap();
    let link = f.root.join("link");
    symlink(&member, &link).unwrap();
    let dangling = f.root.join("dangling");
    symlink(f.root.join("absent"), &dangling).unwrap();
    let ancestor = f.root.join("ancestor");
    symlink(&f.root, &ancestor).unwrap();
    for companions in [
        BTreeMap::new(),
        BTreeMap::from([(17, member.clone()), (0, member.clone())]),
        BTreeMap::from([(17, member.clone()), (0x7fff, member.clone())]),
        BTreeMap::from([(17, link.clone())]),
        BTreeMap::from([(17, dangling)]),
        BTreeMap::from([(17, ancestor.join("member"))]),
        BTreeMap::from([(17, f.root.clone())]),
    ] {
        assert!(validate_family(&directory, &companions).is_err());
        assert!(read_resource(&path, &directory, &companions, 0, 16).is_err());
    }
    assert!(inspect_directory(&link).is_err());
    let directory_link = f.root.join("directory-link");
    symlink(&path, &directory_link).unwrap();
    assert!(inspect_directory(&directory_link).is_err());
    assert!(read_resource(&directory_link, &directory, &good, 0, 16).is_err());
    assert!(inspect_directory(&ancestor.join("directory.vpk")).is_err());
    assert!(inspect_directory(&f.root).is_err());
}

#[test]
fn resource_read_obeys_cap_before_allocation() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    fs::write(
        &path,
        family_vpk(1, &[entry("a", 0x7fff, 0, 3, b"pre")], b"abc"),
    )
    .unwrap();
    let mut directory = inspect_directory(&path).unwrap();
    assert_eq!(
        read_resource(&path, &directory, &BTreeMap::new(), 0, 6).unwrap(),
        b"preabc"
    );
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 5).is_err());
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 0).is_err());
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 1, 16).is_err());
    assert!(read_resource(&path, &directory, &BTreeMap::new(), usize::MAX, 16).is_err());
    directory.resources[0].location.length = u64::MAX;
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 16 * 1024 * 1024).is_err());
    directory.resources[0].location.length = u32::MAX as u64;
    assert!(read_resource(&path, &directory, &BTreeMap::new(), 0, 16 * 1024 * 1024).is_err());
}

#[test]
fn family_tree_and_entry_limits_are_inclusive() {
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    const MAX_TREE: usize = 16 * 1024 * 1024;
    let names: Vec<_> = (0..100001).map(|i| i.to_string()).collect();
    let entries: Vec<_> = names.iter().map(|n| entry(n, 0x7fff, 0, 0, b"")).collect();
    fs::write(&path, family_vpk(1, &entries[..100000], b"")).unwrap();
    assert_eq!(inspect_directory(&path).unwrap().resources.len(), 100000);
    fs::write(&path, family_vpk(1, &entries, b"")).unwrap();
    assert!(inspect_directory(&path).is_err());
    // One extension/directory, 256 distinct resources, with genuine preload data.
    let overhead = 4
        + (0..256)
            .map(|i| i.to_string().len() + 1 + 18)
            .sum::<usize>()
        + 3;
    let mut remaining = MAX_TREE - overhead;
    let mut tree = b"x\0 \0".to_vec();
    for i in 0..256 {
        tree.extend_from_slice(i.to_string().as_bytes());
        tree.push(0);
        let preload = remaining.min(u16::MAX as usize);
        remaining -= preload;
        tree.extend_from_slice(&42u32.to_le_bytes());
        tree.extend_from_slice(&(preload as u16).to_le_bytes());
        tree.extend_from_slice(&0x7fffu16.to_le_bytes());
        tree.extend_from_slice(&[0; 8]);
        tree.extend_from_slice(&0xffffu16.to_le_bytes());
        tree.resize(tree.len() + preload, 0x41);
    }
    tree.extend_from_slice(&[0, 0, 0]);
    assert_eq!(remaining, 0);
    assert_eq!(tree.len(), MAX_TREE);
    fs::write(&path, vpk_with_tree(1, &tree, b"", [&[], &[], &[]])).unwrap();
    assert_eq!(inspect_directory(&path).unwrap().resources.len(), 256);
    tree.push(0);
    fs::write(&path, vpk_with_tree(1, &tree, b"", [&[], &[], &[]])).unwrap();
    assert!(inspect_directory(&path).is_err());
}

#[test]
fn inspection_ignores_sidecars_and_preserves_source_bytes_and_mtimes() {
    use modlock_helper::paths::digest_bytes;
    let f = Fixture::new();
    let path = f.root.join("directory.vpk");
    let member = f.root.join("member");
    let sidecar = f.root.join("directory.vpk.origin");
    let elsewhere = f.root.join("unrelated");
    fs::write(&path, family_vpk(2, &[entry("a", 17, 0, 3, b"pre")], b"")).unwrap();
    fs::write(&member, b"abc").unwrap();
    fs::write(&elsewhere, b"unrelated bytes").unwrap();
    fs::write(&sidecar, elsewhere.to_str().unwrap()).unwrap();
    let paths = [&path, &member, &sidecar, &elsewhere, &f.source];
    let before: Vec<_> = paths
        .iter()
        .map(|p| {
            (
                digest_bytes(&fs::read(p).unwrap()).unwrap(),
                fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();
    let directory = inspect_directory(&path).unwrap();
    assert!(validate_family(&directory, &BTreeMap::new()).is_err());
    let companions = BTreeMap::from([(17, member.clone())]);
    validate_family(&directory, &companions).unwrap();
    assert_eq!(
        read_resource(&path, &directory, &companions, 0, 6).unwrap(),
        b"preabc"
    );
    let after: Vec<_> = paths
        .iter()
        .map(|p| {
            (
                digest_bytes(&fs::read(p).unwrap()).unwrap(),
                fs::metadata(p).unwrap().modified().unwrap(),
            )
        })
        .collect();
    assert_eq!(after, before);
}
