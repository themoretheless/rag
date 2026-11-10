use super::*;
#[test]
fn macro_token_arguments_do_not_fall_back_to_recursive_or_same_name_bindings() {
    let g=graph(&[("/p/lib.rs","fn count_raw(){} macro_rules! defraw{($i:ident)=>{$i()}} fn run(){defraw!(count_raw);count_raw();}")]);
    let edges: Vec<_> = g
        .bound_references
        .iter()
        .filter(|r| r.reference.name == "count_raw")
        .collect();
    assert_eq!(edges.len(), 2);
    let token = edges.iter().find(|e| !e.reference.is_call).unwrap();
    assert!(token.candidates.is_empty());
    assert_eq!(token.resolution_basis, "rust_macro_expansion_unverified");
    assert_eq!(
        edges
            .iter()
            .find(|e| e.reference.is_call)
            .unwrap()
            .candidates
            .len(),
        1
    );
    let g = graph(&[("/p/lib.rs", "mod namespace{} fn run(){namespace();}")]);
    assert!(g.calls[0].candidates.is_empty());
}
fn graph(files: &[(&str, &str)]) -> CodeSnapshot {
    let mut docs = HashMap::new();
    let mut out = CodeSnapshot::default();
    for (i, (path, source)) in files.iter().enumerate() {
        let mut d = parse(&i.to_string(), path, source, 1).unwrap();
        assert!(!d.parse_errors, "{path}: {source}");
        d.project = Some("fixture".into());
        out.symbols.extend(d.symbols.clone());
        docs.insert(i.to_string(), d);
    }
    out.bound_references = resolve::references(&out.symbols, &docs);
    out.calls = out
        .bound_references
        .iter()
        .filter(|r| r.reference.is_call)
        .cloned()
        .collect();
    out
}
fn targets(g: &CodeSnapshot, expression: &str) -> Vec<String> {
    let edges: Vec<_> = g
        .calls
        .iter()
        .filter(|c| c.reference.expression == expression)
        .collect();
    assert_eq!(edges.len(), 1, "{expression}: {:?}", g.calls);
    edges[0]
        .candidates
        .iter()
        .map(|id| {
            g.symbols
                .iter()
                .find(|s| &s.id == id)
                .unwrap()
                .qualified_name
                .clone()
        })
        .collect()
}
const TYPES:&str="struct User; struct Other; struct Service; impl User {fn save(&self){}} impl Other {fn save(&self){}} impl Service {fn user(&self)->User {User} fn other(&self)->Other {Other} async fn user_async(&self)->User {User} fn typed<T>(&self)->T {unimplemented!()} }";
#[test]
fn rust_return_chains_target_exact_owner_and_preserve_inner_calls() {
    let g = graph(&[(
        "/p/src/lib.rs",
        &format!("{TYPES} fn run(s:Service){{s.user().save();s.other().save();}}"),
    )]);
    assert_eq!(targets(&g, "s.user().save"), ["User::save"]);
    assert_eq!(targets(&g, "s.other().save"), ["Other::save"]);
    assert_eq!(targets(&g, "s.user"), ["Service::user"]);
    assert_eq!(targets(&g, "s.other"), ["Service::other"]);
}
#[test]
fn rust_async_requires_await_and_explicit_generic_substitution() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} async fn run(s:Service){{(s.user_async().await).save();s.user_async().save();s.typed::<User>().save();s.typed::<Other>().save();s.typed().save();}}"))]);
    assert_eq!(targets(&g, "(s.user_async().await).save"), ["User::save"]);
    assert!(targets(&g, "s.user_async().save").is_empty());
    assert_eq!(targets(&g, "s.typed::<User>().save"), ["User::save"]);
    assert_eq!(targets(&g, "s.typed::<Other>().save"), ["Other::save"]);
    assert!(targets(&g, "s.typed().save").is_empty());
}
#[test]
fn rust_raw_pointer_trait_receivers_keep_mutability_and_pointee_identity() {
    let source="struct User; struct Other; trait Pointer {fn mark(self);} impl<T> Pointer for *const T {fn mark(self){}} impl<T> Pointer for *mut T {fn mark(self){}} impl User {fn mark(&self){}} fn run(c:*const u8,m:*mut u8,u:*const User,o:*mut Other,r:&User){c.mark();m.mark();u.mark();o.mark();r.mark();(m as *const u8).mark();}";
    let g = graph(&[("/p/lib.rs", source)]);
    for (expression, owner) in [
        ("c.mark", "*const T"),
        ("m.mark", "*mut T"),
        ("u.mark", "*const T"),
        ("o.mark", "*mut T"),
        ("(m as *const u8).mark", "*const T"),
    ] {
        let e = g
            .calls
            .iter()
            .find(|c| c.reference.expression == expression)
            .unwrap();
        assert_eq!(e.candidates.len(), 1, "{expression}: {e:?}");
        let target = g.symbols.iter().find(|s| s.id == e.candidates[0]).unwrap();
        assert!(
            target.qualified_name.contains(owner),
            "{expression}: {target:?}"
        );
    }
    assert_eq!(targets(&g, "r.mark"), ["User::mark"]);
}
#[test]
fn rust_raw_pointer_blanket_impl_does_not_assume_unsized_pointee_is_sized() {
    let g=graph(&[("/p/lib.rs","trait Mark {fn mark(self);} impl<T> Mark for *const T {fn mark(self){}} fn run(s:*const str,a:*const [u8],p:*const u8){s.mark();a.mark();p.mark();}")]);
    assert!(targets(&g, "s.mark").is_empty());
    assert!(targets(&g, "a.mark").is_empty());
    assert_eq!(targets(&g, "p.mark").len(), 1);
}
#[test]
fn rust_raw_pointer_sysroot_methods_do_not_bind_extension_trait_names() {
    let g=graph(&[("/p/lib.rs","trait Shadow {fn is_null(self)->bool;} impl Shadow for *const u8 {fn is_null(self)->bool{false}} fn run(p:*const u8){p.is_null();}")]);
    assert!(targets(&g, "p.is_null").is_empty());
}
#[test]
fn rust_raw_pointers_do_not_autoderef_or_cross_const_mut_or_primitive_types() {
    let g=graph(&[("/p/lib.rs","struct User; impl User {fn save(&self){}} trait Exact {fn exact(self);} impl Exact for *const u8 {fn exact(self){}} fn run(c:*const u8,m:*mut u8,w:*const u16,u:*const User){c.exact();m.exact();w.exact();u.save();}")]);
    assert_eq!(targets(&g, "c.exact").len(), 1);
    for e in ["m.exact", "w.exact", "u.save"] {
        assert!(targets(&g, e).is_empty(), "{e}");
    }
}
#[test]
fn rust_tuple_struct_fields_preserve_positions_and_generic_owners() {
    let g=graph(&[("/p/src/lib.rs", &format!("{TYPES} struct Pair<T>(pub T, Other); struct Outer(Pair<User>); impl Outer {{fn run(&self){{self.0.0.save();self.0.1.save();self.0.2.save();}}}} fn run(p:Pair<User>){{p.0.save();p.1.save();p.2.save();}}"))]);
    for e in ["p.0.save", "self.0.0.save"] {
        assert_eq!(targets(&g, e), ["User::save"], "{e}");
    }
    for e in ["p.1.save", "self.0.1.save"] {
        assert_eq!(targets(&g, e), ["Other::save"], "{e}");
    }
    for e in ["p.2.save", "self.0.2.save"] {
        assert!(targets(&g, e).is_empty(), "{e}");
    }
}
#[test]
fn rust_conditional_tuple_slots_and_enum_variants_do_not_guess_layouts() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} struct Pair(#[cfg(any())] Other, User); enum Variant {{Pair(User,Other)}} fn run(p:Pair,e:Variant){{p.0.save();p.1.save();e.0.save();}}"))]);
    for e in ["p.0.save", "p.1.save", "e.0.save"] {
        assert!(targets(&g, e).is_empty(), "{e}");
    }
}
#[test]
fn rust_fields_arrays_and_pointers_keep_receiver_ownership() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} struct Container{{user:User,other:Other}} fn run(c:Container,users:&[User],others:Vec<Other>,arc:std::sync::Arc<User>,boxed:Box<Other>){{c.user.save();c.other.save();users[0].save();others[0].save();arc.save();boxed.save();}}"))]);
    for e in ["c.user.save", "users[0].save", "arc.save"] {
        assert_eq!(targets(&g, e), ["User::save"], "{e}");
    }
    for e in ["c.other.save", "others[0].save", "boxed.save"] {
        assert_eq!(targets(&g, e), ["Other::save"], "{e}");
    }
}
#[test]
fn rust_local_factory_inference_aliases_and_shadowing() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} type Alias=User; fn factory()->User{{User}} fn run(a:Alias){{let u=factory();u.save();a.save();{{let u=unknown();u.save();}}}}"))]);
    assert_eq!(targets(&g, "a.save"), ["User::save"]);
    let calls: Vec<_> = g
        .calls
        .iter()
        .filter(|c| c.reference.expression == "u.save")
        .collect();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].candidates.len(), 1);
    assert!(calls[1].candidates.is_empty());
    assert_eq!(
        g.symbols
            .iter()
            .find(|s| s.id == calls[0].candidates[0])
            .unwrap()
            .qualified_name,
        "User::save"
    );
}
#[test]
fn rust_return_type_is_resolved_in_declaration_document() {
    let g=graph(&[
            ("/p/src/lib.rs","pub mod api; struct User; impl User{fn save(&self){}} fn run(){crate::api::make().save();}"),
            ("/p/src/api.rs","pub struct User; impl User{pub fn save(&self){}} pub fn make()->User{User}"),
        ]);
    let edge = g
        .calls
        .iter()
        .find(|c| c.reference.expression == "crate::api::make().save")
        .unwrap();
    assert_eq!(edge.candidates.len(), 1, "{edge:?}");
    assert_eq!(
        g.symbols
            .iter()
            .find(|s| s.id == edge.candidates[0])
            .unwrap()
            .path,
        "/p/src/api.rs"
    );
}
#[test]
fn rust_extern_guards_preserve_captured_dependencies_and_cannot_match_local_sentinels() {
    for (declaration, dependency, expression) in [
        ("extern crate vendor;", "vendor", "vendor::work"),
        ("extern crate std as standard;", "std", "standard::work"),
    ] {
        let manifest = format!(
            "[package]\nname='app'\n[dependencies]\n{dependency}={{package='dep',path='../dep'}}\n"
        );
        let source = format!("{declaration}fn run(){{{expression}();}}");
        let g = graph(&[
            ("/w/app/Cargo.toml", &manifest),
            ("/w/app/src/lib.rs", &source),
            ("/w/dep/Cargo.toml", "[package]\nname='dep'\n"),
            ("/w/dep/src/lib.rs", "pub fn work(){}"),
        ]);
        let edge = g
            .calls
            .iter()
            .find(|c| c.reference.expression == expression)
            .unwrap();
        assert_eq!(edge.candidates.len(), 1, "{edge:?}");
        assert_eq!(
            g.symbols
                .iter()
                .find(|s| s.id == edge.candidates[0])
                .unwrap()
                .path,
            "/w/dep/src/lib.rs"
        );
    }
    let g = graph(&[("/p/Cargo.toml", "[package]\nname='fixture'\n[dependencies]\nvendor='1'\n"), ("/p/src/lib.rs", "extern crate vendor as alias;mod __unverified_extern_crate__{pub mod vendor{pub fn work(){}}}fn run(){alias::work();}")]);
    assert!(targets(&g, "alias::work").is_empty());
    let g = graph(&[
        (
            "/p/Cargo.toml",
            "[package]\nname='fixture'\n[dependencies]\nvendor={package='dep',path='dep'}\n",
        ),
        (
            "/p/src/lib.rs",
            "mod vendor{pub fn work(){}}use vendor as alias;fn run(){alias::work();}",
        ),
        ("/p/dep/Cargo.toml", "[package]\nname='dep'\n"),
        ("/p/dep/src/lib.rs", "pub fn work(){}"),
    ]);
    assert!(
        targets(&g, "alias::work").is_empty(),
        "a plain use path must not be reinterpreted as an extern-crate declaration"
    );
}
#[test]
fn rust_standard_wrapper_guards_follow_root_extern_aliases_into_submodules() {
    for (declaration, expected) in [("extern crate std;", 1), ("extern crate vendor as std;", 0)] {
        let root = format!("{declaration}mod child;");
        let g = graph(&[("/p/Cargo.toml", "[package]\nname='fixture'\n[dependencies]\nvendor='1'\n"), ("/p/src/lib.rs", &root), ("/p/src/child.rs", "struct User;impl User{fn save(&self){}}fn run(values:std::vec::Vec<User>){values[0].save();}")]);
        assert_eq!(targets(&g, "values[0].save").len(), expected);
    }
}
#[test]
fn rust_self_uses_captured_lifetime_impl_type_and_preserves_self_returns() {
    let g = graph(&[("/p/src/lib.rs", "struct Worker<'a>{data:&'a [u8]} impl<'a> Worker<'a>{fn finish(&self){} fn run(&self){self.finish();}fn copy(&self)->Self{Self{data:self.data}}}fn check(w:Worker<'_>){w.copy().finish();}")]);
    assert_eq!(targets(&g, "self.finish"), ["Worker<'a>::finish"]);
    assert_eq!(targets(&g, "w.copy().finish"), ["Worker<'a>::finish"]);
}
#[test]
fn rust_impl_types_resolve_in_inline_module_not_same_named_root() {
    let g = graph(&[("/p/src/lib.rs", "struct Item;impl Item{fn finish(&self){}}mod inside{struct Item;impl Item{fn finish(&self){}}fn run(value:Item){value.finish();}}fn run(root:Item){root.finish();}")]);
    assert_eq!(targets(&g, "value.finish"), ["inside::Item::finish"]);
    assert_eq!(targets(&g, "root.finish"), ["Item::finish"]);
}
#[test]
fn rust_self_fields_use_super_paths_from_the_field_declaration_module() {
    let g = graph(&[("/p/Cargo.toml", "[package]\nname='fixture'\n"), ("/p/src/lib.rs", "struct Service;impl Service{fn finish(&self){}}mod providers{pub struct Service;impl Service{pub fn finish(&self){}}}mod inner{struct Worker<'a>{data:&'a [u8],service:super::providers::Service}impl<'a> Worker<'a>{fn run(&self){self.service.finish();}}}")]);
    assert_eq!(
        targets(&g, "self.service.finish"),
        ["providers::Service::finish"]
    );
}
#[test]
fn rust_nested_items_cannot_inherit_self_and_unknown_generic_fields_do_not_guess() {
    let g = graph(&[("/p/src/lib.rs", "struct Owner;impl Owner{fn finish(&self){}fn outer(){fn bad(value:Self){value.finish();}}}struct Wrapper<T>{item:T}impl<T> Wrapper<T>{fn run(&self){self.item.finish();}}struct Other;impl Other{fn finish(&self){}}")]);
    assert!(targets(&g, "value.finish").is_empty());
    assert!(targets(&g, "self.item.finish").is_empty());
}
#[test]
fn rust_custom_wrappers_and_unawaited_unknowns_do_not_gain_std_dispatch() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} struct Arc<T>{{t:T}} struct Vec<T>{{t:T}} fn run(arc:Arc<User>,v:Vec<User>){{arc.save();v[0].save();unknown().save();}}"))]);
    for e in ["arc.save", "v[0].save", "unknown().save"] {
        assert!(targets(&g, e).is_empty(), "{e}");
    }
}
#[test]
fn rust_branch_reassignment_invalidates_outer_initializer() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} fn make()->User{{User}} fn run(flag:bool){{let mut u=make();if flag{{u=unknown();}}u.save();}}"))]);
    assert!(targets(&g, "u.save").is_empty());
}
#[test]
fn rust_generic_parameter_shadows_concrete_same_name_type() {
    let g = graph(&[(
        "/p/src/lib.rs",
        &format!("{TYPES} fn generic<User>(u:User){{u.save();}}"),
    )]);
    assert!(targets(&g, "u.save").is_empty());
}
#[test]
fn rust_associated_functions_are_not_instance_methods() {
    let g=graph(&[("/p/src/lib.rs","struct User; impl User{fn create()->User{User} fn run(&self){create().save();}} impl User{fn save(&self){}} fn main(u:User){u.create().save();}")]);
    assert!(targets(&g, "u.create").is_empty());
    assert!(targets(&g, "u.create().save").is_empty());
    assert!(targets(&g, "create().save").is_empty());
}
#[test]
fn rust_enclosing_generic_and_destructured_values_block_same_named_globals() {
    let g=graph(&[("/p/src/lib.rs",&format!("{TYPES} struct Wrapper<T>{{t:T}} impl<User> Wrapper<User>{{fn run(u:User){{u.save();}}}} fn factory()->User{{User}} fn shadow(){{let (factory,_)=unknown();factory().save();let closure=|factory|factory().other().save();}}"))]);
    for expression in ["u.save", "factory().save", "factory().other().save"] {
        assert!(targets(&g, expression).is_empty(), "{expression}");
    }
}
#[test]
fn rust_impls_resolve_across_files_without_same_named_owner_fallback() {
    let g = graph(&[
        (
            "/p/src/lib.rs",
            "pub mod model; pub mod extensions; pub mod other; fn run(u:model::User){u.save();}",
        ),
        ("/p/src/model.rs", "pub struct User;"),
        (
            "/p/src/extensions.rs",
            "use crate::model::User; impl User{pub fn save(&self){}}",
        ),
        (
            "/p/src/other.rs",
            "pub struct User; impl User{pub fn save(&self){}}",
        ),
    ]);
    let edge = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "u.save")
        .unwrap();
    assert_eq!(edge.candidates.len(), 1, "{edge:?}");
    assert_eq!(
        g.symbols
            .iter()
            .find(|s| s.id == edge.candidates[0])
            .unwrap()
            .path,
        "/p/src/extensions.rs"
    );
}
#[test]
fn rust_generic_impl_substitutes_method_returns_and_fields() {
    let g=graph(&[("/p/src/lib.rs","struct User; struct Other; struct Wrapper<T>{item:T} impl User{fn save(&self){}} impl Other{fn save(&self){}} impl<T> Wrapper<T>{fn get(&self)->&T{&self.item}} fn run(u:Wrapper<User>,o:Wrapper<Other>){u.get().save();o.get().save();u.item.save();}")]);
    assert_eq!(targets(&g, "u.get().save"), ["User::save"]);
    assert_eq!(targets(&g, "o.get().save"), ["Other::save"]);
    assert_eq!(targets(&g, "u.item.save"), ["User::save"]);
}
#[test]
fn rust_bounds_dyn_and_impl_trait_link_declarations_not_runtime_implementors() {
    let g=graph(&[("/p/src/lib.rs","trait Render{fn render(&self);} struct User;impl Render for User{fn render(&self){}} fn generic<T:Render>(u:T){u.render();} fn dynamic(d:&dyn Render){d.render();} fn opaque(p:impl Render){p.render();} fn where_bound<W>(w:W)where W:Render{w.render();}")]);
    for expression in ["u.render", "d.render", "p.render", "w.render"] {
        assert_eq!(targets(&g, expression), ["Render::render"], "{expression}");
    }
}
#[test]
fn rust_ufcs_disambiguates_traits_and_inherent_method_wins_member_lookup() {
    let g=graph(&[("/p/src/lib.rs","trait A{fn run(&self);} trait B{fn run(&self);} struct User;impl A for User{fn run(&self){}} impl B for User{fn run(&self){}} impl User{fn run(&self){}} fn check(u:User){u.run();<User as A>::run(&u);<User as B>::run(&u);}")]);
    let intrinsic = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "u.run")
        .unwrap();
    assert_eq!(intrinsic.candidates.len(), 1, "{intrinsic:?}");
    let a = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "<User as A>::run")
        .unwrap();
    let b = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "<User as B>::run")
        .unwrap();
    assert_eq!(a.candidates.len(), 1, "{a:?}");
    assert_eq!(b.candidates.len(), 1, "{b:?}");
    assert_ne!(a.candidates, b.candidates);
    assert_ne!(a.candidates, intrinsic.candidates);
}
#[test]
fn rust_trait_ambiguity_remains_explicit_and_associated_return_uses_selected_impl() {
    let g=graph(&[("/p/src/lib.rs","trait A{fn run(&self);}trait B{fn run(&self);}struct User;impl A for User{fn run(&self){}}impl B for User{fn run(&self){}} fn ambiguous(u:User){u.run();} trait Provider{type Item;fn get(&self)->Self::Item;}struct Store;impl Provider for Store{type Item=User;fn get(&self)->Self::Item{User}}impl User{fn save(&self){}}fn check(s:Store){s.get().save();}")]);
    let edge = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "u.run")
        .unwrap();
    assert_eq!(edge.resolution, "ambiguous");
    assert_eq!(edge.candidates.len(), 2);
    assert_eq!(targets(&g, "s.get().save"), ["User::save"]);
}
#[test]
fn rust_associated_projections_default_returns_and_generic_bounds() {
    let g=graph(&[("/p/src/lib.rs","trait Provider{type Item;fn get(&self)->Self::Item;}struct User;struct Other;impl Other{fn save(&self){}}impl Provider for User{type Item=Other;fn get(&self)->Self::Item{Other}}fn choose<T:Provider>()->T::Item{panic!()}fn exercise(p:<User as Provider>::Item){choose::<User>().save();p.save();}trait Defaulted{fn same(&self)->Self where Self:Sized{panic!()}}impl Defaulted for User{} impl User{fn save(&self){}}fn check(u:User){u.same().save();}")]);
    assert_eq!(targets(&g, "choose::<User>().save"), ["Other::save"]);
    assert_eq!(targets(&g, "p.save"), ["Other::save"]);
    assert_eq!(targets(&g, "u.same().save"), ["User::save"]);
}
#[test]
fn rust_where_bound_impl_requires_evidence_and_trait_alias_is_visible() {
    let g=graph(&[
            ("/p/src/lib.rs","pub mod tr;use crate::tr::Render as Visible;trait Marker{}struct User;struct Other;struct Wrapper<T>{item:T}impl Marker for User{}impl<T> Wrapper<T> where T:Marker{fn work(&self){}}impl Visible for User{fn render(&self){}}fn run(u:User,good:Wrapper<User>,bad:Wrapper<Other>){u.render();good.work();bad.work();}"),
            ("/p/src/tr.rs","pub trait Render{fn render(&self);}"),
        ]);
    assert_eq!(targets(&g, "u.render"), ["User::render"]);
    assert_eq!(targets(&g, "good.work"), ["Wrapper<T>::work"]);
    assert!(targets(&g, "bad.work").is_empty());
}
#[test]
fn rust_inherent_impl_from_another_cargo_target_is_not_a_method_candidate() {
    let g = graph(&[
        ("/p/Cargo.toml", "[package]\nname='app'\nversion='0.1.0'"),
        (
            "/p/src/lib.rs",
            "pub struct User;impl User{pub fn save(&self){}} pub fn check(user:User){user.save();}",
        ),
        (
            "/p/src/main.rs",
            "use app::User;impl User{fn save(&self){}}fn main(){}",
        ),
    ]);
    let edge = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "user.save")
        .unwrap();
    assert_eq!(edge.candidates.len(), 1, "{edge:?}");
    assert_eq!(
        g.symbols
            .iter()
            .find(|s| s.id == edge.candidates[0])
            .unwrap()
            .path,
        "/p/src/lib.rs"
    );
}
#[test]
fn rust_exhausted_impl_search_never_publishes_a_partial_unique_target() {
    let mut source =
        String::from("struct User;impl User{fn save(&self){}}fn run(user:User){user.save();}\n");
    for i in 0..400 {
        source.push_str(&format!(
            "struct Other{i};impl Other{i}{{fn save(&self){{}}}}\n"
        ));
    }
    let g = graph(&[("/p/src/lib.rs", &source)]);
    let edge = g
        .calls
        .iter()
        .find(|e| e.reference.expression == "user.save")
        .unwrap();
    assert!(edge.candidates.is_empty(), "{edge:?}");
    assert_eq!(edge.resolution_basis, "rust_expression_budget_exhausted");
}
#[test]
fn rust_format_31_payload_is_stale_and_refreshable_not_a_decode_failure() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(&directory.path().join("legacy.duckdb")).unwrap();
    let document = crate::models::Document {
        id: "legacy".into(),
        uri: "file:///p/src/lib.rs".into(),
        source_file: Some("/p/src/lib.rs".into()),
        content: "struct User;impl User{fn save(&self){}}fn run(user:User){user.save();}".into(),
        ..Default::default()
    };
    store.documents().upsert_document(&document).unwrap();
    crate::code_index::CodeIndex::new(&store)
        .index_code()
        .unwrap();
    {
        let connection = store.lock().unwrap();
        let payload: String = connection
            .query_row(
                "SELECT payload FROM code_index_documents WHERE document_id='legacy'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let mut old: serde_json::Value = serde_json::from_str(&payload).unwrap();
        old["version"] = serde_json::json!(31);
        old["rust_semantics"]
            .as_object_mut()
            .unwrap()
            .remove("trait_data");
        for function in old["rust_semantics"]["functions"]
            .as_object_mut()
            .unwrap()
            .values_mut()
        {
            function.as_object_mut().unwrap().remove("bounds");
        }
        connection
            .execute(
                "UPDATE code_index_documents SET payload=? WHERE document_id='legacy'",
                [old.to_string()],
            )
            .unwrap();
    }
    let stale = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    assert_eq!(stale.stale_documents, ["legacy"]);
    assert!(stale.calls.is_empty());
    assert_eq!(
        crate::code_index::CodeIndex::new(&store)
            .index_code()
            .unwrap()
            .indexed,
        1
    );
    let current = crate::code_index::CodeIndex::new(&store)
        .code_snapshot()
        .unwrap();
    assert!(current.stale_documents.is_empty());
    assert_eq!(current.calls[0].candidates.len(), 1);
}
