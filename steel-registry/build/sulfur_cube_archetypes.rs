//! Builds the `minecraft:sulfur_cube_archetype` registry from datapack JSON.
//!
//! Each entry embeds its JSON file; `SulfurCubeArchetype::from_json` decodes
//! it with serde and the same JSON is sent to clients as NBT.

use std::fs;

use heck::ToShoutySnakeCase;
use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;

const DIR: &str = "../steel-utils/build_assets/builtin_datapacks/minecraft/sulfur_cube_archetype";

pub(crate) fn build() -> TokenStream {
    println!("cargo:rerun-if-changed={DIR}");

    let mut names: Vec<String> = fs::read_dir(DIR)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|s| s.to_str()) == Some("json"))
        .map(|path| path.file_stem().unwrap().to_str().unwrap().to_owned())
        .collect();
    names.sort();

    let mut statics = TokenStream::new();
    let mut registers = TokenStream::new();
    for name in &names {
        let ident = Ident::new(&name.to_shouty_snake_case(), Span::call_site());
        let file = format!(
            "/../steel-utils/build_assets/builtin_datapacks/minecraft/sulfur_cube_archetype/{name}.json"
        );
        statics.extend(quote! {
            pub static #ident: LazyLock<SulfurCubeArchetype> = LazyLock::new(|| {
                SulfurCubeArchetype::from_json(
                    Identifier::vanilla_static(#name),
                    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), #file)),
                )
            });
        });
        registers.extend(quote! { registry.register(&*#ident); });
    }

    quote! {
        use std::sync::LazyLock;
        use steel_utils::Identifier;
        use crate::sulfur_cube_archetype::{SulfurCubeArchetype, SulfurCubeArchetypeRegistry};

        #statics

        pub fn register_sulfur_cube_archetypes(registry: &mut SulfurCubeArchetypeRegistry) {
            #registers
        }
    }
}
