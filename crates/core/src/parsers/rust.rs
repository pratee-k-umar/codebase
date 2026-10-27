//! Rust parser using tree-sitter.

use tree_sitter::{Node, Tree};
use crate::{Symbol, Import, Export, SymbolId, SymbolKind, Visibility, Signature, Parameter, TypeRef, TypeParam, Attribute, ImportKind, ImportItem, Result, Language, node_to_span, node_text, LanguageParser};
use std::path::Path;

pub struct RustParser;

impl RustParser {
    pub fn new() -> Self { Self }
}

impl LanguageParser for RustParser {
    fn extract_symbols(&self, tree: &Tree, content: &str, file: &Path) -> Result<Vec<Symbol>> {
        let mut symbols = Vec::new();
        let root = tree.root_node();
        self.extract_symbols_recursive(&root, content, file, None, &mut symbols)?;
        Ok(symbols)
    }

    fn extract_imports(&self, tree: &Tree, content: &str, file: &Path) -> Result<Vec<Import>> {
        let mut imports = Vec::new();
        let root = tree.root_node();
        self.extract_imports_recursive(&root, content, file, &mut imports)?;
        Ok(imports)
    }

    fn extract_exports(&self, tree: &Tree, content: &str, file: &Path) -> Result<Vec<Export>> {
        // Rust: pub = exported
        let mut exports = Vec::new();
        let root = tree.root_node();
        self.extract_exports_recursive(&root, content, file, &mut exports)?;
        Ok(exports)
    }

    fn resolve_type(&self, _node: Node, _content: &str, _scope: &crate::parser::Scope) -> Result<Option<crate::TypeRef>> {
        Ok(None)
    }
}

impl RustParser {
    fn extract_symbols_recursive(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        let mut children = Vec::new();
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                children.push(cursor.node());
                if !cursor.goto_next_sibling() { break; }
            }
        }

        for child in children {
            match child.kind() {
                "function_item" => {
                    if let Some(sym) = self.parse_function(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "struct_item" => {
                    if let Some(sym) = self.parse_struct(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "enum_item" => {
                    if let Some(sym) = self.parse_enum(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "impl_item" => {
                    self.parse_impl(&child, content, file, parent.clone(), symbols)?;
                }
                "trait_item" => {
                    if let Some(sym) = self.parse_trait(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "mod_item" => {
                    if let Some(sym) = self.parse_module(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "const_item" | "static_item" => {
                    self.parse_const_static(&child, content, file, parent.clone(), symbols)?;
                }
                "type_item" => {
                    self.parse_type_alias(&child, content, file, parent.clone(), symbols)?;
                }
                "macro_definition" => {
                    self.parse_macro(&child, content, file, parent.clone(), symbols)?;
                }
                _ => {
                    self.extract_symbols_recursive(&child, content, file, parent.clone(), symbols)?;
                }
            }
        }
        Ok(())
    }

    fn parse_function(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let kind = if parent.is_some() { SymbolKind::Method } else { SymbolKind::Function };
        let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, kind, span);

        // Parameters
        let params_node = node.child_by_field_name("parameters");
        let parameters = params_node.map(|n| self.parse_parameters(&n, content)).unwrap_or_default();

        // Return type
        let return_type_node = node.child_by_field_name("return_type");
        let return_type = return_type_node.and_then(|n| self.parse_type(&n, content));

        // Generics
        let generics_node = node.child_by_field_name("generics");
        let type_parameters = generics_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        // Check for async
        let is_async = node.child_by_field_name("async").is_some();
        
        // Check for const fn
        let _is_const = node.child_by_field_name("const").is_some();

        // Visibility
        let visibility = self.determine_visibility(node, content);

        // Attributes (derive, cfg, etc.)
        let attributes = self.extract_attributes(node, content);

        // Doc comments
        let documentation = self.extract_doc_comments(node, content);

        Ok(Some(Symbol {
            id,
            name,
            kind,
            language: Language::Rust,
            file: file.to_path_buf(),
            span,
            signature: Some(Signature {
                parameters,
                return_type,
                type_parameters,
                is_async,
                is_generator: false,
            }),
            visibility,
            documentation,
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_struct(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, SymbolKind::Struct, span);

        let generics_node = node.child_by_field_name("generics");
        let _type_parameters = generics_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        let visibility = self.determine_visibility(node, content);
        let attributes = self.extract_attributes(node, content);
        let documentation = self.extract_doc_comments(node, content);

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Struct,
            language: Language::Rust,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility,
            documentation,
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_enum(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, SymbolKind::Enum, span);

        let generics_node = node.child_by_field_name("generics");
        let _type_parameters = generics_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        let visibility = self.determine_visibility(node, content);
        let attributes = self.extract_attributes(node, content);
        let documentation = self.extract_doc_comments(node, content);

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Enum,
            language: Language::Rust,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility,
            documentation,
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_impl(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        // impl blocks - extract methods
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            for child in body.named_children(&mut body.walk()) {
                if child.kind() == "function_item" {
                    if let Some(sym) = self.parse_function(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn parse_trait(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, SymbolKind::Trait, span);

        let generics_node = node.child_by_field_name("generics");
        let _type_parameters = generics_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        let visibility = self.determine_visibility(node, content);
        let attributes = self.extract_attributes(node, content);
        let documentation = self.extract_doc_comments(node, content);

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Trait,
            language: Language::Rust,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility,
            documentation,
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_module(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, SymbolKind::Module, span);

        let visibility = self.determine_visibility(node, content);
        let attributes = self.extract_attributes(node, content);
        let documentation = self.extract_doc_comments(node, content);

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Module,
            language: Language::Rust,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility,
            documentation,
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_const_static(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        let name_node = node.child_by_field_name("name");
        let type_node = node.child_by_field_name("type");
        let value_node = node.child_by_field_name("value");
        
        if let Some(name_node) = name_node {
            let name = node_text(name_node, content).to_string();
            let span = node_to_span(*node, content);
            let kind = if node.kind() == "const_item" { SymbolKind::Constant } else { SymbolKind::Variable };
            let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, kind, span);

            let type_ref = type_node.and_then(|n| self.parse_type(&n, content))
                .or_else(|| value_node.as_ref().and_then(|v| self.infer_type_from_value(v, content)));

            let visibility = self.determine_visibility(node, content);
            let documentation = self.extract_doc_comments(node, content);

            symbols.push(Symbol {
                id,
                name,
                kind,
                language: Language::Rust,
                file: file.to_path_buf(),
                span,
                signature: type_ref.map(|t| Signature {
                    parameters: vec![],
                    return_type: Some(t),
                    type_parameters: vec![],
                    is_async: false,
                    is_generator: false,
                }),
                visibility,
                documentation,
                attributes: Vec::new(),
                parent: parent.clone(),
                children: Vec::new(),
            });
        }
        Ok(())
    }

    fn parse_type_alias(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        let name_node = node.child_by_field_name("name");
        let type_node = node.child_by_field_name("type");
        
        if let Some(name_node) = name_node {
            let name = node_text(name_node, content).to_string();
            let span = node_to_span(*node, content);
            let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, SymbolKind::TypeAlias, span);

            let type_ref = type_node.and_then(|n| self.parse_type(&n, content));
            let visibility = self.determine_visibility(node, content);
            let documentation = self.extract_doc_comments(node, content);

            symbols.push(Symbol {
                id,
                name,
                kind: SymbolKind::TypeAlias,
                language: Language::Rust,
                file: file.to_path_buf(),
                span,
                signature: type_ref.map(|t| Signature {
                    parameters: vec![],
                    return_type: Some(t),
                    type_parameters: vec![],
                    is_async: false,
                    is_generator: false,
                }),
                visibility,
                documentation,
                attributes: Vec::new(),
                parent: parent.clone(),
                children: Vec::new(),
            });
        }
        Ok(())
    }

    fn parse_macro(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        let name_node = node.child_by_field_name("name");
        if let Some(name_node) = name_node {
            let name = node_text(name_node, content).to_string();
            let span = node_to_span(*node, content);
            let id = SymbolId::new(Language::Rust, &file.to_path_buf(), &name, SymbolKind::Macro, span);

            let visibility = self.determine_visibility(node, content);
            let documentation = self.extract_doc_comments(node, content);

            symbols.push(Symbol {
                id,
                name,
                kind: SymbolKind::Macro,
                language: Language::Rust,
                file: file.to_path_buf(),
                span,
                signature: None,
                visibility,
                documentation,
                attributes: Vec::new(),
                parent: parent.clone(),
                children: Vec::new(),
            });
        }
        Ok(())
    }

    fn parse_parameters(&self, node: &Node, content: &str) -> Vec<Parameter> {
        let mut params = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "parameter" {
                let pattern_node = child.child_by_field_name("pattern");
                let type_node = child.child_by_field_name("type");
                
                if let Some(pattern) = pattern_node {
                    let name = node_text(pattern, content).to_string();
                    let type_ref = type_node.as_ref().and_then(|n| self.parse_type(n, content));
                    params.push(Parameter { name, type_ref, default_value: None, is_optional: false, is_rest: false });
                }
            } else if child.kind() == "self_parameter" {
                // self, &self, &mut self
                let name = "self".to_string();
                let type_ref = None; // Self type
                params.push(Parameter { name, type_ref, default_value: None, is_optional: false, is_rest: false });
            }
        }
        params
    }

    fn parse_type_parameters(&self, node: &Node, content: &str) -> Vec<TypeParam> {
        let mut params = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "type_parameter" {
                let name_node = child.child_by_field_name("name");
                let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                
                let bounds_node = child.child_by_field_name("bounds");
                let constraint = bounds_node.and_then(|n| self.parse_type(&n, content));
                
                params.push(TypeParam { name, constraint, default: None });
            }
        }
        params
    }

    fn parse_type(&self, node: &Node, content: &str) -> Option<TypeRef> {
        let text = node_text(*node, content).to_string();
        Some(TypeRef { name: text, generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None })
    }

    fn infer_type_from_value(&self, node: &Node, _content: &str) -> Option<TypeRef> {
        match node.kind() {
            "string_literal" | "raw_string_literal" => Some(TypeRef { name: "String".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "integer_literal" => Some(TypeRef { name: "i32".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "float_literal" => Some(TypeRef { name: "f64".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "true" | "false" => Some(TypeRef { name: "bool".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "array_expression" => Some(TypeRef { name: "Vec".into(), generic_args: vec![], is_nullable: false, is_array: true, symbol_id: None }),
            _ => None,
        }
    }

    fn determine_visibility(&self, node: &Node, content: &str) -> Visibility {
        // Check for pub, pub(crate), pub(super), pub(in path)
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                if cursor.node().kind() == "visibility_modifier" {
                    let text = node_text(cursor.node(), content);
                    if text == "pub" {
                        return Visibility::Public;
                    } else if text.starts_with("pub(") {
                        return Visibility::Internal; // pub(crate), pub(super), etc.
                    }
                }
                if !cursor.goto_next_sibling() { break; }
            }
        }
        Visibility::Private
    }

    fn extract_attributes(&self, node: &Node, content: &str) -> Vec<Attribute> {
        let mut attrs = Vec::new();
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                if cursor.node().kind() == "attribute_item" || cursor.node().kind() == "inner_attribute" {
                    let text = node_text(cursor.node(), content);
                    attrs.push(Attribute { 
                        name: text.trim_start_matches('#').trim_start_matches('!').trim_matches(['[', ']']).to_string(),
                        arguments: vec![],
                    });
                }
                if !cursor.goto_next_sibling() { break; }
            }
        }
        attrs
    }

    fn extract_doc_comments(&self, node: &Node, content: &str) -> Option<String> {
        let mut docs = Vec::new();
        let mut cursor = node.walk();
        if cursor.goto_previous_sibling() {
            loop {
                if cursor.node().kind() == "line_comment" || cursor.node().kind() == "block_comment" {
                    let text = node_text(cursor.node(), content);
                    if text.starts_with("///") || text.starts_with("//!") || text.starts_with("/**") {
                        docs.push(text.trim_start_matches("///").trim_start_matches("//!").trim_start_matches("/**").trim_end_matches("*/").trim().to_string());
                    } else {
                        break;
                    }
                } else {
                    break;
                }
                if !cursor.goto_previous_sibling() { break; }
            }
        }
        if docs.is_empty() { None } else { Some(docs.join("\n")) }
    }

    fn extract_imports_recursive(&self, node: &Node, content: &str, file: &Path, imports: &mut Vec<Import>) -> Result<()> {
        match node.kind() {
            "use_declaration" => {
                if let Some(imp) = self.parse_use(node, content, file)? {
                    imports.push(imp);
                }
            }
            "extern_crate_declaration" => {
                // extern crate foo;
                if let Some(imp) = self.parse_extern_crate(node, content, file)? {
                    imports.push(imp);
                }
            }
            _ => {}
        }

        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                self.extract_imports_recursive(&cursor.node(), content, file, imports)?;
                if !cursor.goto_next_sibling() { break; }
            }
        }
        Ok(())
    }

    fn parse_use(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Import>> {
        let span = node_to_span(*node, content);
        let mut items = Vec::new();

        // use path::to::item;
        // use path::{item1, item2};
        // use path::*;
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                let child = cursor.node();
                match child.kind() {
                    "scoped_identifier" => {
                        // path::to::item
                        let path = node_text(child, content).to_string();
                        items.push(ImportItem { name: path.clone(), alias: None, symbol_id: None });
                    }
                    "use_wildcard" => {
                        // *
                        items.push(ImportItem { name: "*".into(), alias: None, symbol_id: None });
                    }
                    "use_list" => {
                        // { item1, item2 }
                        for item in child.named_children(&mut child.walk()) {
                            if item.kind() == "use_as_clause" {
                                let name_node = item.child_by_field_name("name");
                                let alias_node = item.child_by_field_name("alias");
                                let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                                let alias = alias_node.map(|n| node_text(n, content)).map(|s| s.to_string());
                                if !name.is_empty() {
                                    items.push(ImportItem { name, alias, symbol_id: None });
                                }
                            } else if item.kind() == "scoped_identifier" {
                                let name = node_text(item, content).to_string();
                                items.push(ImportItem { name: name.clone(), alias: None, symbol_id: None });
                            }
                        }
                    }
                    _ => {}
                }
                if !cursor.goto_next_sibling() { break; }
            }
        }

        if items.is_empty() { return Ok(None); }

        Ok(Some(Import {
            file: file.to_path_buf(),
            span,
            module: items[0].name.clone(),
            kind: ImportKind::Static,
            items,
            is_type_only: false,
        }))
    }

    fn parse_extern_crate(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Import>> {
        let span = node_to_span(*node, content);
        let name_node = node.child_by_field_name("name");
        if let Some(name_node) = name_node {
            let name = node_text(name_node, content).to_string();
            let items = vec![ImportItem { name: name.clone(), alias: None, symbol_id: None }];
            Ok(Some(Import {
                file: file.to_path_buf(),
                span,
                module: name,
                kind: ImportKind::Static,
                items,
                is_type_only: false,
            }))
        } else {
            Ok(None)
        }
    }

    fn extract_exports_recursive(&self, node: &Node, content: &str, file: &Path, exports: &mut Vec<Export>) -> Result<()> {
        // In Rust, exports are determined by pub visibility
        // We'll generate exports from public symbols during analysis
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                self.extract_exports_recursive(&cursor.node(), content, file, exports)?;
                if !cursor.goto_next_sibling() { break; }
            }
        }
        Ok(())
    }
}

impl Default for RustParser {
    fn default() -> Self { Self::new() }
}