//! Private, durable local recovery of completed output after budget draining.
use std::{collections::BTreeMap, fs::OpenOptions, io::Write, path::PathBuf};

pub fn preserve(outputs: &BTreeMap<String, String>) -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "a2a-budget-failure-{}",
        bridge_core::ids::AttemptId::mint()
            .map_err(std::io::Error::other)?
            .as_str()
    ));
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(&dir)?;
    let mut manifest = BTreeMap::new();
    for (index, (node, output)) in outputs.iter().enumerate() {
        // Node IDs are data, never path components. Files contain the exact UTF-8 bytes.
        let name = format!("{index}.output");
        write_private(&dir.join(&name), output.as_bytes())?;
        manifest.insert(node, name);
    }
    write_private(&dir.join("completed.json"), &serde_json::to_vec(&manifest)?)?;
    #[cfg(unix)]
    std::fs::File::open(&dir)?.sync_all()?;
    Ok(dir)
}

fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_bytes_and_untrusted_names() {
        for outputs in [
            BTreeMap::new(),
            BTreeMap::from([("../escape".into(), "\0雪\r\n\nlast".into())]),
        ] {
            let path = preserve(&outputs).unwrap();
            let manifest: BTreeMap<String, String> =
                serde_json::from_slice(&std::fs::read(path.join("completed.json")).unwrap())
                    .unwrap();
            assert_eq!(manifest.len(), outputs.len());
            for (node, file) in manifest {
                assert_eq!(
                    std::fs::read(path.join(file)).unwrap(),
                    outputs[&node].as_bytes()
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o700
                );
            }
            std::fs::remove_dir_all(path).unwrap();
        }
    }
}
