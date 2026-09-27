//! Controls for the ADR-0041 slice 2B2b2b2 mount-point census (task §4, §6.2).
//!
//! `mountinfo_*` parse fixture text as bytes, `census_*` drive the two-call count census with
//! fake calls, `within_*` is the component-boundary predicate, and `live_*` run the platform's
//! own census. The exporter's census controls are in `custody_export::tests` (`census_*`).

use super::*;
use std::collections::BTreeMap;

/// One well-formed record whose mount point is the raw field `mount_point`.
fn record(mount_point: &[u8]) -> Vec<u8> {
    let mut line = b"36 35 98:0 / ".to_vec();
    line.extend_from_slice(mount_point);
    line.extend_from_slice(b" rw,noatime shared:1 master:2 - ext4 /dev/root rw\n");
    line
}

fn table(records: &[Vec<u8>]) -> Vec<u8> {
    records.concat()
}

#[test]
fn mountinfo_01_returns_exact_bytes_for_0xff_every_escape_and_an_escaped_backslash() {
    let text = [
        b"871 684 0:124 / / rw,relatime - overlay overlay rw,lowerdir=/a:/b\n".to_vec(),
        // A non-UTF-8 byte passes through verbatim.
        record(b"/mnt/\xff\xfe-raw"),
        // Every supported escape.
        record(br"/mnt/a\040b\011c\012d\134e"),
        // An escaped backslash followed by the text `040` is a literal backslash, then `040`.
        record(br"/mnt/literal\134040"),
        // No optional fields at all.
        b"880 871 0:38 /docker/volumes/x /lsp-target rw,noatime - btrfs /dev/vdb1 rw\n".to_vec(),
    ]
    .concat();
    assert_eq!(
        parse_mountinfo_v1(&text),
        Ok(vec![
            b"/".to_vec(),
            b"/mnt/\xff\xfe-raw".to_vec(),
            b"/mnt/a b\tc\nd\\e".to_vec(),
            br"/mnt/literal\040".to_vec(),
            b"/lsp-target".to_vec(),
        ])
    );
    assert_eq!(parse_mountinfo_v1(b""), Ok(Vec::new()));
}

#[test]
fn mountinfo_02_a_malformed_escape_refuses() {
    for escape in [
        &br"/mnt/\041"[..],
        br"/mnt/\100x",
        br"/mnt/\04",
        br"/mnt/\0",
        br"/mnt/a\",
        br"/mnt/\\040",
        br"/mnt/\x20",
        br"/mnt/\1340\13",
    ] {
        let text = table(&[record(b"/"), record(escape)]);
        assert_eq!(
            parse_mountinfo_v1(&text),
            Err(CustodyMountErrorV1::MalformedEscape { line: 2 }),
            "{}",
            String::from_utf8_lossy(escape)
        );
    }
}

#[test]
fn mountinfo_03_a_malformed_line_refuses() {
    let rows: [(&str, &[u8]); 12] = [
        (
            "no separator",
            b"36 35 98:0 / /mnt rw shared:1 ext4 /dev/root rw\n",
        ),
        (
            "separator too early",
            b"36 35 98:0 / - /mnt rw ext4 /dev/root rw\n",
        ),
        (
            "too few after separator",
            b"36 35 98:0 / /mnt rw - ext4 /dev/root\n",
        ),
        (
            "non-decimal id",
            b"3x 35 98:0 / /mnt rw - ext4 /dev/root rw\n",
        ),
        (
            "non-decimal parent",
            b"36 -1 98:0 / /mnt rw - ext4 /dev/root rw\n",
        ),
        (
            "device without minor",
            b"36 35 98 / /mnt rw - ext4 /dev/root rw\n",
        ),
        (
            "device with three parts",
            b"36 35 9:8:0 / /mnt rw - ext4 /dev/root rw\n",
        ),
        ("empty root", b"36 35 98:0  /mnt rw - ext4 /dev/root rw\n"),
        (
            "relative mount point",
            b"36 35 98:0 / mnt rw - ext4 /dev/root rw\n",
        ),
        ("empty options", b"36 35 98:0 / /mnt  - ext4 /dev/root rw\n"),
        ("empty line", b"\n"),
        (
            "no final newline",
            b"36 35 98:0 / /mnt rw - ext4 /dev/root rw",
        ),
    ];
    for (label, line) in rows {
        let text = [record(b"/"), line.to_vec()].concat();
        assert_eq!(
            parse_mountinfo_v1(&text),
            Err(CustodyMountErrorV1::MalformedLine { line: 2 }),
            "{label}"
        );
    }
}

#[test]
fn mountinfo_04_the_table_bound_admits_its_maximum_and_refuses_one_byte_more() {
    let line = |options: usize| {
        let mut line = b"1 1 0:1 / /m rw".to_vec();
        line.extend(std::iter::repeat_n(b'x', options));
        line.extend_from_slice(b" - t s o\n");
        line
    };
    let base = line(0).len();
    let exact = line(MOUNTINFO_MAX_BYTES_V1 - base);
    assert_eq!(exact.len(), MOUNTINFO_MAX_BYTES_V1);
    assert_eq!(parse_mountinfo_v1(&exact), Ok(vec![b"/m".to_vec()]));
    let over = line(MOUNTINFO_MAX_BYTES_V1 - base + 1);
    assert_eq!(
        parse_mountinfo_v1(&over),
        Err(CustodyMountErrorV1::Oversize)
    );
}

#[test]
fn census_01_the_count_census_retries_one_change_and_refuses_a_second() {
    let listing = |counted: usize| -> Result<Vec<Vec<u8>>, CustodyMountErrorV1> {
        Ok((0..counted)
            .map(|index| format!("/m{index}").into_bytes())
            .collect())
    };
    // A stable count stands on the first attempt.
    let mut counts = [3_usize, 3].into_iter();
    assert_eq!(
        census_by_count(|| Ok(counts.next().expect("two counts")), listing),
        listing(3)
    );
    // A mount that appeared after the fill is seen by the second count, and retried.
    let mut counts = [3_usize, 4, 4, 4].into_iter();
    assert_eq!(
        census_by_count(|| Ok(counts.next().expect("four counts")), listing),
        listing(4)
    );
    // A mount that vanished before the fill fills fewer records, and is retried.
    let mut counts = [3_usize, 2, 2].into_iter();
    let mut fills = [2_usize, 2].into_iter();
    assert_eq!(
        census_by_count(
            || Ok(counts.next().expect("three counts")),
            |_| listing(fills.next().expect("two fills"))
        ),
        listing(2)
    );
    // A second change refuses.
    let mut counts = [3_usize, 4, 4, 5].into_iter();
    assert_eq!(
        census_by_count(|| Ok(counts.next().expect("four counts")), listing),
        Err(CustodyMountErrorV1::Unstable)
    );
    // An oversize count refuses before any fill, and a failed call refuses.
    assert_eq!(
        census_by_count(
            || Ok(MAX_MOUNT_POINTS_V1 + 1),
            |_| panic!("no fill past the bound")
        ),
        Err(CustodyMountErrorV1::Oversize)
    );
    assert_eq!(
        census_by_count(
            || Err(CustodyMountErrorV1::Io(io::ErrorKind::PermissionDenied)),
            listing
        ),
        Err(CustodyMountErrorV1::Io(io::ErrorKind::PermissionDenied))
    );
}

#[test]
fn within_01_containment_is_a_proper_prefix_on_component_boundaries() {
    let rows: [(&[u8], &[u8], bool); 10] = [
        (b"/repo/target", b"/repo", true),
        (b"/repo/a/b", b"/repo", true),
        (b"/repo/\xff", b"/repo", true),
        (b"/repo", b"/repo", false),
        (b"/repository", b"/repo", false),
        (b"/rep", b"/repo", false),
        (b"/", b"/repo", false),
        (b"/other/repo", b"/repo", false),
        (b"/x", b"/", true),
        (b"/", b"/", false),
    ];
    for (mount_point, root, within) in rows {
        assert_eq!(
            mount_point_within(mount_point, root),
            within,
            "{} in {}",
            String::from_utf8_lossy(mount_point),
            String::from_utf8_lossy(root)
        );
    }
}

/// An empty platform census fails closed, a platform refusal passes through, and the injected seam
/// replaces the platform census call by call.
#[test]
fn census_02_an_empty_census_fails_closed_and_the_seam_counts_calls() {
    let seam = seam::install(Box::new(|call| match call {
        1 => Ok(vec![b"/".to_vec()]),
        2 => Ok(Vec::new()),
        _ => Err(CustodyMountErrorV1::Io(io::ErrorKind::NotFound)),
    }));
    assert_eq!(mount_points_v1(), Ok(vec![b"/".to_vec()]));
    assert_eq!(mount_points_v1(), Err(CustodyMountErrorV1::Empty));
    assert_eq!(
        mount_points_v1(),
        Err(CustodyMountErrorV1::Io(io::ErrorKind::NotFound))
    );
    assert_eq!(seam.calls(), 3);
    drop(seam);
    // Injected text runs through the production parser.
    let _seam = seam::install_mountinfo(record(br"/mnt/a\040b"));
    assert_eq!(mount_points_v1(), Ok(vec![b"/mnt/a b".to_vec()]));
}

/// The container lane's own mount table parses, and lists `/`.
#[cfg(target_os = "linux")]
#[test]
fn live_01_the_linux_mount_table_parses_and_lists_the_root() {
    let mount_points = mount_points_v1().expect("the live mount table parses");
    assert!(mount_points.iter().any(|mount_point| mount_point == b"/"));
}

/// The host lane's `getfsstat` census lists `/` (task §6.2).
#[cfg(target_os = "macos")]
#[test]
fn live_01_getfsstat_lists_the_root() {
    let mount_points = mount_points_v1().expect("the getfsstat census");
    assert!(mount_points.iter().any(|mount_point| mount_point == b"/"));
}

/// The new-unsafe inventory, in the style of 2B2a's A14 control: the census adds exactly the
/// `getfsstat` boundary, three sites in two functions, and no other file this slice changes holds
/// a new `unsafe`.
#[test]
fn unsafe_inventory_pins_the_getfsstat_boundary() {
    use syn::visit::Visit as _;

    #[derive(Default)]
    struct UnsafeInventory {
        functions: Vec<String>,
        counts: BTreeMap<String, usize>,
    }

    impl<'ast> syn::visit::Visit<'ast> for UnsafeInventory {
        fn visit_expr_unsafe(&mut self, node: &'ast syn::ExprUnsafe) {
            let name = self.functions.last().expect("unsafe belongs to a function");
            *self.counts.entry(name.clone()).or_default() += 1;
            syn::visit::visit_expr_unsafe(self, node);
        }

        fn visit_macro(&mut self, node: &'ast syn::Macro) {
            // Macro arguments stay tokenized, so an `unsafe` block inside one is counted here.
            let count = node.tokens.to_string().matches("unsafe {").count();
            if count != 0 {
                let name = self.functions.last().expect("macro belongs to a function");
                *self.counts.entry(name.clone()).or_default() += count;
            }
            syn::visit::visit_macro(self, node);
        }

        fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
            self.functions.push(node.sig.ident.to_string());
            syn::visit::visit_block(self, &node.block);
            self.functions.pop();
        }

        fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
            self.functions.push(node.sig.ident.to_string());
            syn::visit::visit_block(self, &node.block);
            self.functions.pop();
        }
    }

    fn inventory(source: &str) -> BTreeMap<String, usize> {
        let parsed = syn::parse_file(source).expect("parse Rust source");
        let mut visitor = UnsafeInventory::default();
        visitor.visit_file(&parsed);
        visitor.counts
    }

    // The count call, and the fill's zeroed buffer and call. Both are compiled only on macOS, and
    // the inventory reads the source, so every lane pins them.
    assert_eq!(
        inventory(include_str!("custody_mounts.rs")),
        BTreeMap::from([
            ("getfsstat_count".to_owned(), 1),
            ("getfsstat_fill".to_owned(), 2),
        ])
    );
    assert_eq!(
        inventory(include_str!("custody_mounts_tests.rs")),
        BTreeMap::new()
    );
    // The exporter keeps exactly 2B2's one site, and the planner holds none.
    assert_eq!(
        inventory(include_str!("custody_export.rs")),
        BTreeMap::from([("preflight_scratch_root".to_owned(), 1)])
    );
    assert_eq!(
        inventory(include_str!("custody_coverage.rs")),
        BTreeMap::new()
    );
}
