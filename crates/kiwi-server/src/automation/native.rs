//! Native app sources shipped with Kiwi. Credentials and downloaded inputs are
//! extracted privately by the runner; they never enter this embedded manifest.
use std::{collections::BTreeMap, path::Path};

const RAMBLER: &[(&str, &str)] = &[
    (
        "build.sh",
        include_str!("../../../../experiments/gboard-rambler/build.sh"),
    ),
    (
        "make-sample.sh",
        include_str!("../../../../experiments/gboard-rambler/make-sample.sh"),
    ),
    (
        "extract-caller.py",
        include_str!("../../../../experiments/gboard-rambler/extract-caller.py"),
    ),
    (
        "Sources/App.swift",
        include_str!("../../../../experiments/gboard-rambler/Sources/App.swift"),
    ),
    (
        "Sources/CLI.swift",
        include_str!("../../../../experiments/gboard-rambler/Sources/CLI.swift"),
    ),
    (
        "Sources/RamblerCore.swift",
        include_str!("../../../../experiments/gboard-rambler/Sources/RamblerCore.swift"),
    ),
    (
        "Sources/OggOpus.swift",
        include_str!("../../../../experiments/gboard-rambler/Sources/OggOpus.swift"),
    ),
    (
        "Tests/ProtocolTests.swift",
        include_str!("../../../../experiments/gboard-rambler/Tests/ProtocolTests.swift"),
    ),
];

pub(super) fn install(root: &Path) -> Result<(), String> {
    let directory = root.join("native");
    std::fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let files: BTreeMap<_, _> = RAMBLER.iter().copied().collect();
    let mut gboard = files.clone();
    gboard.extend([
        (
            "build-gboard.sh",
            include_str!("../../../../experiments/gboard-rambler/build-gboard.sh"),
        ),
        (
            "inject-gboard.py",
            include_str!("../../../../experiments/gboard-rambler/inject-gboard.py"),
        ),
        (
            "Gboard/Bridge.h",
            include_str!("../../../../experiments/gboard-rambler/Gboard/Bridge.h"),
        ),
        (
            "Gboard/Hooks.m",
            include_str!("../../../../experiments/gboard-rambler/Gboard/Hooks.m"),
        ),
        (
            "Gboard/SpeechActivity.swift",
            include_str!("../../../../experiments/gboard-rambler/Gboard/SpeechActivity.swift"),
        ),
        (
            "Gboard/Session.swift",
            include_str!("../../../../experiments/gboard-rambler/Gboard/Session.swift"),
        ),
    ]);
    for (name, sources) in [("rambler", files), ("gboard-rambler", gboard)] {
        let bytes = serde_json::to_vec(&sources).map_err(|e| e.to_string())?;
        let path = directory.join(format!("{name}.json"));
        if std::fs::read(&path).ok().as_deref() != Some(bytes.as_slice()) {
            let temporary = directory.join(format!("{name}.json.new"));
            std::fs::write(&temporary, bytes).map_err(|e| e.to_string())?;
            std::fs::rename(temporary, path).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}
