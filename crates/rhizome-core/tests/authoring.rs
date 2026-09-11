use kb_contract::{
    DomainId, NoteFrontmatter, NoteKind, NoteStatus, RegistryLocator, render_note, resolve_registry,
};
use rhizome_core::author::{AuthorRequest, apply_author, plan_author};
use rhizome_core::{SourceContext, discover_source};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_SCRATCH: AtomicU64 = AtomicU64::new(0);
fn init_git(path: &Path) {
    let output = std::process::Command::new("git")
        .args(["init", "--quiet"])
        .arg(path)
        .output()
        .expect("git should be installed for authoring fixtures");
    assert!(output.status.success(), "git init should succeed");
}

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let n = NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("rhizome-task8-author-{}-{n}", std::process::id()));
        fs::create_dir_all(&path).expect("scratch directory should be created");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn note(description: &str) -> NoteFrontmatter {
    NoteFrontmatter {
        description: description.into(),
        keywords: vec!["fixture".into()],
        kind: NoteKind::Note,
        links: Vec::new(),
        code: Vec::new(),
        assets: Vec::new(),
        supersedes: None,
        status: None::<NoteStatus>,
    }
}

fn source_context(scratch: &Scratch) -> SourceContext {
    let repo = scratch.path().join("repo");
    fs::create_dir_all(repo.join("docs")).expect("domain should be created");
    fs::write(
        repo.join("docs/INDEX.md"),
        b"---\ndescription: docs\nkeywords: [fixture]\nkind: index\n---\n# Docs\n",
    )
    .expect("domain index should be written");
    init_git(&repo);

    let registry_path = scratch.path().join("sources.toml");
    let repo_text = repo.to_string_lossy().replace('\\', "/");
    fs::write(
        &registry_path,
        format!("[[source]]\nname = \"knowledge\"\npath = \"{repo_text}\"\nsurface = \"core\"\n"),
    )
    .expect("registry should be written");
    let registry = resolve_registry(&RegistryLocator {
        explicit: Some(registry_path),
        cwd: scratch.path().to_path_buf(),
        env_path: None,
        workspace_root: None,
        user_config: scratch.path().join("unused.toml"),
    })
    .expect("registry fixture should resolve");
    SourceContext {
        source: registry
            .sources
            .get("knowledge")
            .expect("source should exist")
            .clone(),
        git_root: repo,
        registry_origin: registry.origin,
    }
}

#[test]
fn author_plan_normalizes_unicode_slug_and_writes_logical_identity() {
    let scratch = Scratch::new();
    let context = source_context(&scratch);
    let domain = DomainId::new("docs").expect("fixture domain should be valid");
    let request = AuthorRequest {
        context: context.clone(),
        domain,
        slug: "cafe\u{301}".into(),
        frontmatter: note("Unicode-safe authored note"),
        body: b"# Cafe\n\nbody\n".to_vec(),
    };

    let plan = plan_author(&request).expect("authoring should produce a typed plan");
    apply_author(&plan).expect("author plan should apply");
    let bytes = fs::read(context.source.root.join("docs/caf\u{e9}.md"))
        .expect("authored note should exist");
    assert!(bytes.starts_with(b"---\ndescription: \"Unicode-safe authored note\""));
    assert!(bytes.ends_with(b"# Cafe\n\nbody\n"));
    let snapshot = discover_source(&context).expect("authored source should remain discoverable");
    let authored = snapshot
        .notes
        .iter()
        .find(|note| note.locator.slug == "caf\u{e9}")
        .expect("authored note should have a logical locator");
    assert_eq!(
        authored.locator.identity.to_string(),
        "knowledge:docs:caf\u{e9}"
    );
}

#[test]
fn author_rejects_unsafe_slug_before_touching_source() {
    let scratch = Scratch::new();
    let context = source_context(&scratch);
    let request = AuthorRequest {
        context: context.clone(),
        domain: DomainId::new("docs").expect("fixture domain should be valid"),
        slug: "bad/name".into(),
        frontmatter: note("must not write"),
        body: b"body\n".to_vec(),
    };
    let error = plan_author(&request).expect_err("a slash is not a safe slug");
    assert!(error.to_string().contains("slug"));
    assert!(!context.source.root.join("bad/name.md").exists());
}

#[test]
fn author_never_overwrites_an_existing_note() {
    let scratch = Scratch::new();
    let context = source_context(&scratch);
    let target = context.source.root.join("docs/existing.md");
    let before = render_note(&note("existing"), b"original\n");
    fs::write(&target, &before).expect("existing note should be written");
    let request = AuthorRequest {
        context,
        domain: DomainId::new("docs").expect("fixture domain should be valid"),
        slug: "existing".into(),
        frontmatter: note("must not overwrite"),
        body: b"replacement\n".to_vec(),
    };

    assert!(
        plan_author(&request).is_err(),
        "existing destination must fail closed"
    );
    assert_eq!(
        fs::read(target).expect("existing note should remain"),
        before
    );
}
