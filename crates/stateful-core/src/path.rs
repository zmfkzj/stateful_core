pub fn normalize_relative_path(path: impl AsRef<str>) -> String {
    path.as_ref()
        .replace('\\', "/")
        .split('/')
        .filter(|segment| !segment.is_empty() && *segment != ".")
        .fold(Vec::new(), |mut segments, segment| {
            if segment == ".." {
                segments.pop();
            } else {
                segments.push(segment);
            }
            segments
        })
        .join("/")
}

pub fn normalize_directory_path(path: impl AsRef<str>) -> String {
    let normalized = normalize_relative_path(path);
    if normalized.is_empty() {
        normalized
    } else {
        format!("{normalized}/")
    }
}

pub fn normalized_relative_path_is_empty(path: impl AsRef<str>) -> bool {
    normalize_relative_path(path.as_ref().trim()).is_empty()
}
