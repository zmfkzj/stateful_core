use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn check_paths_for_dependency_shadowing<'a, I>(
    repo_root: &Path,
    paths: I,
) -> anyhow::Result<()>
where
    I: IntoIterator<Item = &'a str>,
{
    let paths = paths.into_iter().collect::<Vec<_>>();
    check_python_paths_for_dependency_shadowing(repo_root, &paths)?;
    check_rust_paths_for_dependency_shadowing(repo_root, &paths)?;
    check_node_paths_for_dependency_shadowing(repo_root, &paths)?;
    Ok(())
}

pub(crate) fn audit_dependency_shadowing(repo_root: &Path) -> anyhow::Result<()> {
    audit_python_import_shadowing(repo_root)?;
    audit_rust_import_shadowing(repo_root)?;
    audit_node_import_shadowing(repo_root)?;
    Ok(())
}

fn check_python_paths_for_dependency_shadowing(
    repo_root: &Path,
    paths: &[&str],
) -> anyhow::Result<()> {
    let protected = python_protected_import_roots(repo_root)?;
    if protected.is_empty() {
        return Ok(());
    }

    for path in paths.iter().copied() {
        let Some(candidate) = import_root_from_relative_path(path) else {
            continue;
        };
        let Some(protection) = protected.get(&candidate.import_root) else {
            continue;
        };
        if repo_root.join(candidate.top_level_entry).exists() {
            continue;
        }

        anyhow::bail!(
            "dependency shadowing guard: refusing to create top-level Python import root '{}' because it shadows pyproject dependency '{}'. Use installed dependencies instead of local shims.",
            candidate.import_root,
            protection,
        );
    }

    Ok(())
}

fn audit_python_import_shadowing(repo_root: &Path) -> anyhow::Result<()> {
    let protected = python_protected_import_roots(repo_root)?;
    if protected.is_empty() {
        return Ok(());
    }

    for entry in fs::read_dir(repo_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if ignore_top_level_entry(&name) {
            continue;
        }

        let path = entry.path();
        let import_root = if path.is_dir() {
            if !path.join("__init__.py").is_file() && !directory_contains_python_file(&path)? {
                continue;
            }
            python_import_identifier(&name).map(str::to_string)
        } else if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("py") {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .and_then(python_import_identifier)
                .map(str::to_string)
        } else {
            None
        };

        let Some(import_root) = import_root else {
            continue;
        };
        let Some(protection) = protected.get(&import_root) else {
            continue;
        };

        anyhow::bail!(
            "dependency shadowing guard: top-level Python import root '{}' shadows pyproject dependency '{}'. Remove or rename the local shim so Python resolves the intended module.",
            import_root,
            protection,
        );
    }

    Ok(())
}

struct ImportRootCandidate {
    import_root: String,
    top_level_entry: String,
}

fn import_root_from_relative_path(path: &str) -> Option<ImportRootCandidate> {
    let path = path.trim().replace('\\', "/");
    let mut components = path.split('/').filter(|segment| !segment.is_empty());
    let first = components.next()?;
    if ignore_top_level_entry(first) {
        return None;
    }

    if components.next().is_some() {
        return python_import_identifier(first).map(|import_root| ImportRootCandidate {
            import_root: import_root.to_string(),
            top_level_entry: first.to_string(),
        });
    }

    let path = Path::new(first);
    if path.extension().and_then(|ext| ext.to_str()) != Some("py") {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    python_import_identifier(stem).map(|import_root| ImportRootCandidate {
        import_root: import_root.to_string(),
        top_level_entry: first.to_string(),
    })
}

fn python_protected_import_roots(repo_root: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let pyproject = repo_root.join("pyproject.toml");
    let contents = match fs::read_to_string(&pyproject) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };

    let dependencies = parse_pyproject_dependency_names(&contents);
    if dependencies.is_empty()
        && !contents.contains("[project]")
        && !contents.contains("[tool.poetry")
    {
        return Ok(BTreeMap::new());
    }

    let mut protected = BTreeMap::new();
    for distribution in dependencies {
        protected.insert(import_root_for_distribution(&distribution), distribution);
    }

    Ok(protected)
}

#[derive(Debug, Clone)]
struct RustManifest {
    root: PathBuf,
    protected: BTreeMap<String, String>,
}

fn check_rust_paths_for_dependency_shadowing(
    repo_root: &Path,
    paths: &[&str],
) -> anyhow::Result<()> {
    let manifests = rust_manifests(repo_root)?;
    if manifests.is_empty() {
        return Ok(());
    }

    for path in paths.iter().copied() {
        let target = repo_root.join(normalized_relative_path(path)?);
        for manifest in &manifests {
            let Some(candidate) = rust_shadow_candidate(&manifest.root, &target) else {
                continue;
            };
            let Some(dependency) = manifest.protected.get(&candidate.import_root) else {
                continue;
            };
            if candidate.local_root.exists() {
                continue;
            }

            anyhow::bail!(
                "dependency shadowing guard: refusing to create Rust module root '{}' because it shadows Cargo dependency '{}'. Use the declared crate dependency instead of a local shim.",
                candidate.import_root,
                dependency,
            );
        }
    }

    Ok(())
}

fn audit_rust_import_shadowing(repo_root: &Path) -> anyhow::Result<()> {
    for manifest in rust_manifests(repo_root)? {
        let src = manifest.root.join("src");
        if !src.is_dir() {
            continue;
        }
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let import_root =
                if path.is_file() && path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(str::to_string)
                } else if path.is_dir() && path.join("mod.rs").is_file() {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .map(str::to_string)
                } else {
                    None
                };
            let Some(import_root) = import_root else {
                continue;
            };
            let Some(dependency) = manifest.protected.get(&import_root) else {
                continue;
            };

            anyhow::bail!(
                "dependency shadowing guard: Rust module root '{}' shadows Cargo dependency '{}'. Remove or rename the local shim so Rust resolves the intended crate.",
                import_root,
                dependency,
            );
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct RustShadowCandidate {
    import_root: String,
    local_root: PathBuf,
}

fn rust_shadow_candidate(manifest_root: &Path, target: &Path) -> Option<RustShadowCandidate> {
    let relative = target.strip_prefix(manifest_root).ok()?;
    let components = normal_components(relative)?;
    if components.len() == 2 && components[0] == "src" {
        let file = Path::new(&components[1]);
        if file.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            let import_root = file.file_stem()?.to_str()?.to_string();
            let local_root = manifest_root.join("src").join(&components[1]);
            return Some(RustShadowCandidate {
                import_root,
                local_root,
            });
        }
    }
    if components.len() >= 3 && components[0] == "src" && components[2] == "mod.rs" {
        let import_root = components[1].clone();
        let local_root = manifest_root.join("src").join(&import_root);
        return Some(RustShadowCandidate {
            import_root,
            local_root,
        });
    }
    None
}

fn rust_manifests(repo_root: &Path) -> anyhow::Result<Vec<RustManifest>> {
    let mut manifests = Vec::new();
    for manifest in find_files_named(repo_root, "Cargo.toml")? {
        let contents = fs::read_to_string(&manifest)?;
        let protected = parse_cargo_dependency_import_roots(&contents);
        if protected.is_empty() {
            continue;
        }
        let Some(root) = manifest.parent() else {
            continue;
        };
        manifests.push(RustManifest {
            root: root.to_path_buf(),
            protected,
        });
    }
    Ok(manifests)
}

fn parse_cargo_dependency_import_roots(contents: &str) -> BTreeMap<String, String> {
    let mut section = String::new();
    let mut dependencies = BTreeMap::new();

    for raw_line in contents.lines() {
        let line = strip_toml_comment(raw_line).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if let Some(header) = toml_table_header(&line) {
            section = header.to_string();
            if let Some(dependency) = cargo_dependency_from_table_header(header) {
                dependencies.insert(import_root_for_distribution(&dependency), dependency);
            }
            continue;
        }
        if !is_cargo_dependency_section(&section) {
            continue;
        }
        let Some((key, _value)) = line.split_once('=') else {
            continue;
        };
        let Some(dependency) = clean_toml_key(key) else {
            continue;
        };
        if dependency != "workspace" {
            dependencies.insert(import_root_for_distribution(&dependency), dependency);
        }
    }

    dependencies
}

fn is_cargo_dependency_section(section: &str) -> bool {
    matches!(
        section,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    ) || (section.starts_with("target.") && section.ends_with(".dependencies"))
}

fn cargo_dependency_from_table_header(header: &str) -> Option<String> {
    for prefix in ["dependencies.", "dev-dependencies.", "build-dependencies."] {
        if let Some(dependency) = header.strip_prefix(prefix) {
            return clean_toml_key(dependency);
        }
    }
    let (_prefix, dependency) = header.split_once(".dependencies.")?;
    clean_toml_key(dependency)
}

fn clean_toml_key(key: &str) -> Option<String> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if key.starts_with('"') || key.starts_with('\'') {
        return toml_quoted_strings(key).into_iter().next();
    }
    Some(key.trim_matches('"').trim_matches('\'').to_string())
}

#[derive(Debug, Clone)]
struct NodePackage {
    root: PathBuf,
    protected: BTreeMap<Vec<String>, String>,
    base_urls: Vec<PathBuf>,
    path_aliases: BTreeSet<String>,
}

fn check_node_paths_for_dependency_shadowing(
    repo_root: &Path,
    paths: &[&str],
) -> anyhow::Result<()> {
    let packages = node_packages(repo_root)?;
    if packages.is_empty() {
        return Ok(());
    }

    for path in paths.iter().copied() {
        let target = repo_root.join(normalized_relative_path(path)?);
        for package in &packages {
            let Some(candidate) = node_shadow_candidate(package, &target) else {
                continue;
            };
            if candidate.local_root.exists() {
                continue;
            }

            anyhow::bail!(
                "dependency shadowing guard: refusing to create JavaScript/TypeScript import root '{}' because it shadows package dependency '{}'. Use the package dependency instead of a local shim.",
                candidate.import_root,
                candidate.dependency,
            );
        }
    }

    Ok(())
}

fn audit_node_import_shadowing(repo_root: &Path) -> anyhow::Result<()> {
    for package in node_packages(repo_root)? {
        for base_url in &package.base_urls {
            let base = package.root.join(base_url);
            if !base.is_dir() {
                continue;
            }
            for (segments, dependency) in &package.protected {
                if package.path_aliases.contains(dependency) {
                    continue;
                }
                if existing_node_shadow_root(&base, segments).is_some() {
                    anyhow::bail!(
                        "dependency shadowing guard: JavaScript/TypeScript import root '{}' shadows package dependency '{}'. Remove or rename the local shim so the resolver uses the package dependency.",
                        dependency,
                        dependency,
                    );
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct NodeShadowCandidate {
    import_root: String,
    dependency: String,
    local_root: PathBuf,
}

fn node_shadow_candidate(package: &NodePackage, target: &Path) -> Option<NodeShadowCandidate> {
    let relative = target.strip_prefix(&package.root).ok()?;
    let components = normal_components(relative)?;
    if components.first().map(String::as_str) == Some("node_modules") {
        let package_components = &components[1..];
        for (segments, dependency) in &package.protected {
            if component_prefix_matches(package_components, segments) {
                return Some(NodeShadowCandidate {
                    import_root: dependency.clone(),
                    dependency: dependency.clone(),
                    local_root: package.root.join("node_modules").join(segments.join("/")),
                });
            }
        }
        return None;
    }

    for base_url in &package.base_urls {
        let base = package.root.join(base_url);
        let relative = target.strip_prefix(&base).ok()?;
        let components = normal_components(relative)?;
        for (segments, dependency) in &package.protected {
            if package.path_aliases.contains(dependency) {
                continue;
            }
            let Some(local_root) = node_shadow_local_root(&base, &components, segments) else {
                continue;
            };
            return Some(NodeShadowCandidate {
                import_root: dependency.clone(),
                dependency: dependency.clone(),
                local_root,
            });
        }
    }

    None
}

fn existing_node_shadow_root(base: &Path, segments: &[String]) -> Option<PathBuf> {
    let root = base.join(segments.join("/"));
    if root.is_dir() {
        return Some(root);
    }
    if segments.len() == 1 {
        for extension in JS_TS_EXTENSIONS {
            let file = base.join(format!("{}.{}", segments[0], extension));
            if file.is_file() {
                return Some(file);
            }
        }
    }
    None
}

fn node_shadow_local_root(
    base: &Path,
    components: &[String],
    package_segments: &[String],
) -> Option<PathBuf> {
    if component_prefix_matches(components, package_segments) {
        return Some(base.join(package_segments.join("/")));
    }
    if package_segments.len() == 1 && components.len() == 1 {
        let file = Path::new(&components[0]);
        if JS_TS_EXTENSIONS.contains(&file.extension()?.to_str()?) {
            let stem = file.file_stem()?.to_str()?;
            if stem == package_segments[0] {
                return Some(base.join(&components[0]));
            }
        }
    }
    None
}

fn component_prefix_matches(components: &[String], prefix: &[String]) -> bool {
    components.len() >= prefix.len()
        && components
            .iter()
            .zip(prefix)
            .all(|(component, expected)| component == expected)
}

fn node_packages(repo_root: &Path) -> anyhow::Result<Vec<NodePackage>> {
    let mut packages = Vec::new();
    for manifest in find_files_named(repo_root, "package.json")? {
        let contents = fs::read_to_string(&manifest)?;
        let protected = parse_package_dependency_imports(&contents);
        if protected.is_empty() {
            continue;
        }
        let Some(root) = manifest.parent() else {
            continue;
        };
        let (base_urls, path_aliases) = node_resolution_config(root)?;
        packages.push(NodePackage {
            root: root.to_path_buf(),
            protected,
            base_urls,
            path_aliases,
        });
    }
    Ok(packages)
}

fn parse_package_dependency_imports(contents: &str) -> BTreeMap<Vec<String>, String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(contents) else {
        return BTreeMap::new();
    };
    let mut dependencies = BTreeMap::new();
    for section in [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ] {
        let Some(object) = value.get(section).and_then(serde_json::Value::as_object) else {
            continue;
        };
        for package in object.keys() {
            if let Some(segments) = node_package_segments(package) {
                dependencies.insert(segments, package.clone());
            }
        }
    }
    dependencies
}

fn node_resolution_config(root: &Path) -> anyhow::Result<(Vec<PathBuf>, BTreeSet<String>)> {
    let mut base_urls = Vec::new();
    let mut path_aliases = BTreeSet::new();
    for config_name in ["tsconfig.json", "jsconfig.json"] {
        let path = root.join(config_name);
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) else {
            continue;
        };
        let Some(compiler_options) = value
            .get("compilerOptions")
            .and_then(serde_json::Value::as_object)
        else {
            continue;
        };
        if let Some(base_url) = compiler_options
            .get("baseUrl")
            .and_then(serde_json::Value::as_str)
        {
            base_urls.push(PathBuf::from(base_url));
        }
        if let Some(paths) = compiler_options
            .get("paths")
            .and_then(serde_json::Value::as_object)
        {
            for alias in paths.keys() {
                path_aliases.insert(alias.trim_end_matches("/*").to_string());
            }
        }
    }
    base_urls.sort();
    base_urls.dedup();
    Ok((base_urls, path_aliases))
}

fn node_package_segments(package: &str) -> Option<Vec<String>> {
    let mut parts = package.split('/');
    let first = parts.next()?.trim();
    if first.is_empty() {
        return None;
    }
    if first.starts_with('@') {
        let second = parts.next()?.trim();
        if second.is_empty() || parts.next().is_some() {
            return None;
        }
        return Some(vec![first.to_string(), second.to_string()]);
    }
    if parts.next().is_some() {
        return None;
    }
    Some(vec![first.to_string()])
}

fn parse_pyproject_dependency_names(contents: &str) -> BTreeSet<String> {
    let lines = contents.lines().collect::<Vec<_>>();
    let mut section = String::new();
    let mut dependencies = BTreeSet::new();
    let mut index = 0;

    while index < lines.len() {
        let line = strip_toml_comment(lines[index]).trim().to_string();
        if line.is_empty() {
            index += 1;
            continue;
        }
        if let Some(header) = toml_table_header(&line) {
            section = header.to_string();
            index += 1;
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            index += 1;
            continue;
        };
        let key = key.trim();
        let value = value.trim();

        if section == "project" && key == "dependencies" {
            let (value, end_index) = collect_toml_array_value(&lines, index, value);
            add_requirement_strings(&value, &mut dependencies);
            index = end_index + 1;
            continue;
        }
        if section == "project.optional-dependencies" || section == "dependency-groups" {
            let (value, end_index) = collect_toml_array_value(&lines, index, value);
            add_requirement_strings(&value, &mut dependencies);
            index = end_index + 1;
            continue;
        }
        if section == "tool.poetry.dependencies"
            || (section.starts_with("tool.poetry.group.") && section.ends_with(".dependencies"))
        {
            let dependency = key.trim_matches('"').trim_matches('\'');
            if dependency != "python" && !dependency.is_empty() {
                dependencies.insert(dependency.to_string());
            }
        }

        index += 1;
    }

    dependencies
}

fn collect_toml_array_value(lines: &[&str], start: usize, first_value: &str) -> (String, usize) {
    let mut value = first_value.to_string();
    let mut depth = toml_bracket_delta(first_value);
    let mut index = start;
    while depth > 0 && index + 1 < lines.len() {
        index += 1;
        let line = strip_toml_comment(lines[index]);
        depth += toml_bracket_delta(&line);
        value.push('\n');
        value.push_str(&line);
    }
    (value, index)
}

fn add_requirement_strings(value: &str, dependencies: &mut BTreeSet<String>) {
    for requirement in toml_quoted_strings(value) {
        if let Some(name) = requirement_distribution_name(&requirement) {
            dependencies.insert(name);
        }
    }
}

fn requirement_distribution_name(requirement: &str) -> Option<String> {
    let requirement = requirement.trim();
    let mut name = String::new();
    for character in requirement.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
            name.push(character);
        } else {
            break;
        }
    }
    if name.is_empty() { None } else { Some(name) }
}

fn import_root_for_distribution(distribution: &str) -> String {
    let mut root = String::new();
    let mut previous_separator = false;
    for character in distribution.chars() {
        if character.is_ascii_alphanumeric() {
            root.push(character.to_ascii_lowercase());
            previous_separator = false;
        } else if matches!(character, '-' | '_' | '.') && !previous_separator {
            root.push('_');
            previous_separator = true;
        }
    }
    root.trim_matches('_').to_string()
}

fn python_import_identifier(value: &str) -> Option<&str> {
    let mut chars = value.chars();
    let first = chars.next()?;
    if first != '_' && !first.is_ascii_alphabetic() {
        return None;
    }
    if chars.all(|character| character == '_' || character.is_ascii_alphanumeric()) {
        Some(value)
    } else {
        None
    }
}

fn directory_contains_python_file(path: &Path) -> anyhow::Result<bool> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        if entry.path().extension().and_then(|ext| ext.to_str()) == Some("py") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn toml_table_header(line: &str) -> Option<&str> {
    let line = line.trim();
    if line.starts_with("[[") || !line.starts_with('[') {
        return None;
    }
    let end = line.find(']')?;
    Some(line[1..end].trim())
}

fn strip_toml_comment(line: &str) -> String {
    let mut result = String::new();
    let mut in_basic = false;
    let mut in_literal = false;
    let mut escaped = false;

    for character in line.chars() {
        if in_basic {
            result.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_basic = false;
            }
            continue;
        }
        if in_literal {
            result.push(character);
            if character == '\'' {
                in_literal = false;
            }
            continue;
        }
        match character {
            '"' => {
                in_basic = true;
                result.push(character);
            }
            '\'' => {
                in_literal = true;
                result.push(character);
            }
            '#' => break,
            _ => result.push(character),
        }
    }

    result
}

fn toml_bracket_delta(value: &str) -> i32 {
    let mut delta = 0;
    let mut in_basic = false;
    let mut in_literal = false;
    let mut escaped = false;

    for character in value.chars() {
        if in_basic {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                in_basic = false;
            }
            continue;
        }
        if in_literal {
            if character == '\'' {
                in_literal = false;
            }
            continue;
        }
        match character {
            '"' => in_basic = true,
            '\'' => in_literal = true,
            '[' => delta += 1,
            ']' => delta -= 1,
            _ => {}
        }
    }

    delta
}

fn toml_quoted_strings(value: &str) -> Vec<String> {
    let mut strings = Vec::new();
    let mut chars = value.chars().peekable();

    while let Some(character) = chars.next() {
        if character != '"' && character != '\'' {
            continue;
        }
        let quote = character;
        let mut string = String::new();
        let mut escaped = false;
        for character in chars.by_ref() {
            if quote == '"' && escaped {
                string.push(character);
                escaped = false;
                continue;
            }
            if quote == '"' && character == '\\' {
                escaped = true;
                continue;
            }
            if character == quote {
                break;
            }
            string.push(character);
        }
        strings.push(string);
    }

    strings
}

fn ignore_top_level_entry(name: &str) -> bool {
    name.starts_with('.')
        || matches!(
            name,
            "__pycache__" | "build" | "dist" | "node_modules" | "target" | "tmp" | "venv" | ".venv"
        )
}

fn normalized_relative_path(path: &str) -> anyhow::Result<PathBuf> {
    let path = path.trim().replace('\\', "/");
    let mut normalized = PathBuf::new();
    for component in Path::new(&path).components() {
        match component {
            std::path::Component::Normal(segment) => normalized.push(segment),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                anyhow::bail!("dependency shadowing guard target path escapes the repo");
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                anyhow::bail!("dependency shadowing guard target path must be repo-relative");
            }
        }
    }
    Ok(normalized)
}

fn normal_components(path: &Path) -> Option<Vec<String>> {
    let mut components = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(segment) => {
                components.push(segment.to_str()?.to_string());
            }
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir
            | std::path::Component::RootDir
            | std::path::Component::Prefix(_) => return None,
        }
    }
    Some(components)
}

fn find_files_named(repo_root: &Path, file_name: &str) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_files_named(repo_root, file_name, &mut files)?;
    Ok(files)
}

fn collect_files_named(
    dir: &Path,
    file_name: &str,
    files: &mut Vec<PathBuf>,
) -> anyhow::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if file_type.is_file() && name == file_name {
            files.push(entry.path());
        } else if file_type.is_dir() && should_descend_directory(&name) {
            collect_files_named(&entry.path(), file_name, files)?;
        }
    }
    Ok(())
}

fn should_descend_directory(name: &str) -> bool {
    !name.starts_with('.')
        && !matches!(
            name,
            "node_modules" | "target" | "tmp" | "venv" | ".venv" | "__pycache__"
        )
}

const JS_TS_EXTENSIONS: &[&str] = &["js", "jsx", "ts", "tsx", "mjs", "cjs", "mts", "cts"];

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_repo(name: &str) -> std::path::PathBuf {
        let repo = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        if repo.exists() {
            fs::remove_dir_all(&repo).expect("old temp repo should be removable");
        }
        fs::create_dir_all(&repo).expect("temp repo should be creatable");
        repo
    }

    #[test]
    fn denies_rust_dependency_shadow_from_cargo_manifest() {
        let repo = temp_repo("stateful-shadow-rust");
        fs::create_dir_all(repo.join("src")).expect("src should be creatable");
        fs::write(
            repo.join("Cargo.toml"),
            r#"
[package]
name = "example"
version = "0.1.0"

[dependencies]
serde-json = "1"
"#,
        )
        .expect("Cargo.toml should write");

        let error = check_paths_for_dependency_shadowing(&repo, ["src/serde_json.rs"])
            .expect_err("rust crate shadow should be denied");

        let message = error.to_string();
        assert!(message.contains("dependency shadowing guard"));
        assert!(message.contains("serde_json"));
        assert!(message.contains("serde-json"));
        fs::remove_dir_all(repo).expect("temp repo should be removable");
    }

    #[test]
    fn denies_typescript_dependency_shadow_from_tsconfig_base_url() {
        let repo = temp_repo("stateful-shadow-typescript");
        fs::create_dir_all(repo.join("src/@scope")).expect("src scope should be creatable");
        fs::write(
            repo.join("package.json"),
            r#"{"dependencies":{"@scope/pkg":"1.0.0"}}"#,
        )
        .expect("package.json should write");
        fs::write(
            repo.join("tsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"src"}}"#,
        )
        .expect("tsconfig should write");

        let error = check_paths_for_dependency_shadowing(&repo, ["src/@scope/pkg/index.ts"])
            .expect_err("typescript package shadow should be denied");

        let message = error.to_string();
        assert!(message.contains("dependency shadowing guard"));
        assert!(message.contains("@scope/pkg"));
        fs::remove_dir_all(repo).expect("temp repo should be removable");
    }

    #[test]
    fn denies_javascript_dependency_shadow_from_jsconfig_base_url() {
        let repo = temp_repo("stateful-shadow-javascript");
        fs::write(
            repo.join("package.json"),
            r#"{"dependencies":{"lodash":"^4.17.0"}}"#,
        )
        .expect("package.json should write");
        fs::write(
            repo.join("jsconfig.json"),
            r#"{"compilerOptions":{"baseUrl":"."}}"#,
        )
        .expect("jsconfig should write");

        let error = check_paths_for_dependency_shadowing(&repo, ["lodash.js"])
            .expect_err("javascript package shadow should be denied");

        let message = error.to_string();
        assert!(message.contains("dependency shadowing guard"));
        assert!(message.contains("lodash"));
        fs::remove_dir_all(repo).expect("temp repo should be removable");
    }
}
