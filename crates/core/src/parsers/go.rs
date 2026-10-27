//! Go parser using tree-sitter.

use tree_sitter::{Node, Tree};
use crate::{Symbol, Import, Export, SymbolId, SymbolKind, Visibility, Signature, Parameter, TypeRef, TypeParam, Attribute, ImportKind, ImportItem, Result, Language, node_to_span, node_text, LanguageParser};
use std::path::Path;

pub struct GoParser;

impl GoParser {
    pub fn new() -> Self { Self }
}

impl LanguageParser for GoParser {
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
        // Go: exported = capitalized name
        let mut exports = Vec::new();
        let root = tree.root_node();
        self.extract_exports_recursive(&root, content, file, &mut exports)?;
        Ok(exports)
    }

    fn resolve_type(&self, _node: Node, _content: &str, _scope: &crate::parser::Scope) -> Result<Option<crate::TypeRef>> {
        Ok(None)
    }
}

impl GoParser {
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
                "function_declaration" => {
                    if let Some(sym) = self.parse_function(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "method_declaration" => {
                    if let Some(sym) = self.parse_method(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "type_declaration" => {
                    self.parse_type_decl(&child, content, file, parent.clone(), symbols)?;
                }
                "var_declaration" | "const_declaration" => {
                    self.parse_var_const(&child, content, file, parent.clone(), symbols)?;
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
        let id = SymbolId::new(Language::Go, &file.to_path_buf(), &name, SymbolKind::Function, span);

        // Parameters
        let params_node = node.child_by_field_name("parameters");
        let parameters = params_node.map(|n| self.parse_parameters(&n, content)).unwrap_or_default();

        // Return types (Go supports multiple return values)
        let result_node = node.child_by_field_name("result");
        let return_type = result_node.and_then(|n| self.parse_type(&n, content));

        // Check for generic type parameters
        let type_params_node = node.child_by_field_name("type_parameters");
        let type_parameters = type_params_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        // Visibility: capitalized = public
        let visibility = if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            Visibility::Public
        } else {
            Visibility::Private
        };

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Function,
            language: Language::Go,
            file: file.to_path_buf(),
            span,
            signature: Some(Signature {
                parameters,
                return_type,
                type_parameters,
                is_async: false,
                is_generator: false,
            }),
            visibility,
            documentation: self.extract_doc_comment(node, content),
            attributes: Vec::new(),
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_method(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::Go, &file.to_path_buf(), &name, SymbolKind::Method, span);

        let receiver_node = node.child_by_field_name("receiver");
        let receiver_type = receiver_node.and_then(|n| self.parse_type(&n, content));

        let params_node = node.child_by_field_name("parameters");
        let parameters = params_node.map(|n| self.parse_parameters(&n, content)).unwrap_or_default();

        let result_node = node.child_by_field_name("result");
        let return_type = result_node.and_then(|n| self.parse_type(&n, content));

        let type_params_node = node.child_by_field_name("type_parameters");
        let type_parameters = type_params_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        let visibility = if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            Visibility::Public
        } else {
            Visibility::Private
        };

        let mut attributes = Vec::new();
        if let Some(rcv) = receiver_type {
            attributes.push(Attribute { name: "receiver".into(), arguments: vec![rcv.name] });
        }

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Method,
            language: Language::Go,
            file: file.to_path_buf(),
            span,
            signature: Some(Signature {
                parameters,
                return_type,
                type_parameters,
                is_async: false,
                is_generator: false,
            }),
            visibility,
            documentation: self.extract_doc_comment(node, content),
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_type_decl(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        // type TypeName = ... or type TypeName struct { ... }
        let spec_list = node.child_by_field_name("specs");
        if let Some(specs) = spec_list {
            for spec in specs.named_children(&mut specs.walk()) {
                if spec.kind() == "type_spec" {
                    let name_node = spec.child_by_field_name("name");
                    let type_node = spec.child_by_field_name("type");
                    
                    if let Some(name_node) = name_node {
                        let name = node_text(name_node, content).to_string();
                        let span = node_to_span(spec, content);
                        
                        // Determine if struct, interface, or alias
                        let (kind, type_ref) = if let Some(type_node) = type_node {
                            match type_node.kind() {
                                "struct_type" => (SymbolKind::Struct, self.parse_struct_type(&type_node, content)),
                                "interface_type" => (SymbolKind::Interface, self.parse_interface_type(&type_node, content)),
                                _ => (SymbolKind::TypeAlias, self.parse_type(&type_node, content)),
                            }
                        } else {
                            (SymbolKind::TypeAlias, None)
                        };

                        let visibility = if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                            Visibility::Public
                        } else {
                            Visibility::Private
                        };

                        let id = SymbolId::new(Language::Go, &file.to_path_buf(), &name, kind, span);
                        symbols.push(Symbol {
                            id,
                            name,
                            kind,
                            language: Language::Go,
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
                            documentation: self.extract_doc_comment(&spec, content),
                            attributes: Vec::new(),
                            parent: parent.clone(),
                            children: Vec::new(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn parse_var_const(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        let kind = if node.kind() == "const_declaration" { SymbolKind::Constant } else { SymbolKind::Variable };
        
        let spec_list = node.child_by_field_name("specs");
        if let Some(specs) = spec_list {
            for spec in specs.named_children(&mut specs.walk()) {
                if spec.kind() == "var_spec" || spec.kind() == "const_spec" {
                    let name_nodes: Vec<Node> = spec.children_by_field_name("name", &mut spec.walk()).collect();
                    let type_node = spec.child_by_field_name("type");
                    let value_nodes: Vec<Node> = spec.children_by_field_name("value", &mut spec.walk()).collect();
                    
                    for (i, name_node) in name_nodes.into_iter().enumerate() {
                        let name = node_text(name_node, content).to_string();
                        let span = node_to_span(name_node, content);
                        let visibility = if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
                            Visibility::Public
                        } else {
                            Visibility::Private
                        };
                        
                        let type_ref = type_node.and_then(|n| self.parse_type(&n, content))
                            .or_else(|| value_nodes.get(i).and_then(|v| self.infer_type_from_value(v, content)));
                        
                        let id = SymbolId::new(Language::Go, &file.to_path_buf(), &name, kind, span);
                        symbols.push(Symbol {
                            id,
                            name,
                            kind,
                            language: Language::Go,
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
                            documentation: self.extract_doc_comment(&spec, content),
                            attributes: Vec::new(),
                            parent: parent.clone(),
                            children: Vec::new(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn parse_parameters(&self, node: &Node, content: &str) -> Vec<Parameter> {
        let mut params = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "parameter_declaration" {
                let name_nodes: Vec<Node> = child.children_by_field_name("name", &mut child.walk()).collect();
                let type_node = child.child_by_field_name("type");
                
                for name_node in name_nodes {
                    let name = node_text(name_node, content).to_string();
                    let type_ref = type_node.as_ref().and_then(|n| self.parse_type(n, content));
                    params.push(Parameter { name, type_ref, default_value: None, is_optional: false, is_rest: false });
                }
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
                
                let constraint_node = child.child_by_field_name("constraint");
                let constraint = constraint_node.and_then(|n| self.parse_type(&n, content));
                
                params.push(TypeParam { name, constraint, default: None });
            }
        }
        params
    }

    fn parse_type(&self, node: &Node, content: &str) -> Option<TypeRef> {
        let text = node_text(*node, content).to_string();
        Some(TypeRef { name: text, generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None })
    }

    fn parse_struct_type(&self, _node: &Node, _content: &str) -> Option<TypeRef> {
        Some(TypeRef { name: "struct".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None })
    }

    fn parse_interface_type(&self, _node: &Node, _content: &str) -> Option<TypeRef> {
        Some(TypeRef { name: "interface".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None })
    }

    fn infer_type_from_value(&self, node: &Node, content: &str) -> Option<TypeRef> {
        match node.kind() {
            "string_literal" | "raw_string_literal" => Some(TypeRef { name: "string".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "int_literal" => Some(TypeRef { name: "int".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "float_literal" => Some(TypeRef { name: "float64".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "true" | "false" => Some(TypeRef { name: "bool".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "composite_literal" => {
                // Could be struct, array, slice, map
                let type_node = node.child_by_field_name("type");
                type_node.and_then(|n| self.parse_type(&n, content))
            }
            _ => None,
        }
    }

    fn extract_doc_comment(&self, node: &Node, content: &str) -> Option<String> {
        // Look for preceding comment
        let mut cursor = node.walk();
        if cursor.goto_previous_sibling() {
            if cursor.node().kind() == "comment" {
                let text = node_text(cursor.node(), content);
                if text.starts_with("//") {
                    return Some(text.trim_start_matches("//").trim().to_string());
                }
            }
        }
        None
    }

    fn extract_imports_recursive(&self, node: &Node, content: &str, file: &Path, imports: &mut Vec<Import>) -> Result<()> {
        if node.kind() == "import_declaration" {
            if let Some(imp) = self.parse_import(node, content, file)? {
                imports.push(imp);
            }
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

    fn parse_import(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Import>> {
        let span = node_to_span(*node, content);
        let mut items = Vec::new();

        let spec_list = node.child_by_field_name("specs");
        if let Some(specs) = spec_list {
            for spec in specs.named_children(&mut specs.walk()) {
                if spec.kind() == "import_spec" {
                    let name_node = spec.child_by_field_name("path");
                    let alias_node = spec.child_by_field_name("name");
                    
                    let path = name_node.map(|n| node_text(n, content).trim_matches('"')).unwrap_or("").to_string();
                    let alias = alias_node.map(|n| node_text(n, content)).map(|s| s.to_string());
                    
                    if !path.is_empty() {
                        items.push(ImportItem { name: path.clone(), alias, symbol_id: None });
                    }
                }
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

    fn extract_exports_recursive(&self, node: &Node, content: &str, file: &Path, exports: &mut Vec<Export>) -> Result<()> {
        // In Go, exports are implicit (capitalized names)
        // We'll generate export entries from symbols
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

impl Default for GoParser {
    fn default() -> Self { Self::new() }
}