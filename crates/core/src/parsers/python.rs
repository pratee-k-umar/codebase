//! Python parser using tree-sitter.

use tree_sitter::{Node, Tree};
use crate::{Symbol, Import, Export, SymbolId, SymbolKind, Visibility, Signature, Parameter, TypeRef, TypeParam, Attribute, ImportKind, ImportItem, ExportKind, ExportItem, Result, Language, node_to_span, node_text, LanguageParser};
use std::path::Path;

pub struct PythonParser;

impl PythonParser {
    pub fn new() -> Self { Self }
}

impl LanguageParser for PythonParser {
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
        // Python doesn't have explicit exports - everything at module level is exported
        // We'll infer from __all__ if present
        let mut exports = Vec::new();
        let root = tree.root_node();
        self.extract_exports_recursive(&root, content, file, &mut exports)?;
        Ok(exports)
    }

    fn resolve_type(&self, _node: Node, _content: &str, _scope: &crate::parser::Scope) -> Result<Option<crate::TypeRef>> {
        Ok(None)
    }
}

impl PythonParser {
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
                "function_definition" => {
                    if let Some(sym) = self.parse_function(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "class_definition" => {
                    if let Some(sym) = self.parse_class(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "decorated_definition" => {
                    // Handle @decorator on functions/classes
                    self.extract_symbols_recursive(&child, content, file, parent.clone(), symbols)?;
                }
                "assignment" => {
                    // Module-level variables
                    if parent.is_none() {
                        self.parse_assignments(&child, content, file, symbols)?;
                    }
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
        let id = SymbolId::new(Language::Python, &file.to_path_buf(), &name, kind, span);

        // Parameters
        let params_node = node.child_by_field_name("parameters");
        let parameters = params_node.map(|n| self.parse_parameters(&n, content)).unwrap_or_default();

        // Return type annotation
        let return_type_node = node.child_by_field_name("return_type");
        let return_type = return_type_node.and_then(|n| self.parse_type_annotation(&n, content));

        // Type parameters (PEP 695)
        let type_params_node = node.child_by_field_name("type_parameters");
        let type_parameters = type_params_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        // Check for async
        let is_async = node.child_by_field_name("async").is_some();
        
        // Check for generator (yield)
        let is_generator = self.has_yield(node);

        // Decorators
        let attributes = self.extract_decorators(node, content);

        // Docstring
        let documentation = self.extract_docstring(node, content);

        // Visibility: _private, __dunder
        let visibility = if name.starts_with("__") && !name.ends_with("__") {
            Visibility::Private
        } else if name.starts_with("_") {
            Visibility::Protected
        } else {
            Visibility::Public
        };

        Ok(Some(Symbol {
            id,
            name,
            kind,
            language: Language::Python,
            file: file.to_path_buf(),
            span,
            signature: Some(Signature { parameters, return_type, type_parameters, is_async, is_generator }),
            visibility,
            documentation,
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_class(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::Python, &file.to_path_buf(), &name, SymbolKind::Class, span);

        // Base classes
        let bases_node = node.child_by_field_name("superclasses");
        let mut attributes = Vec::new();
        if let Some(bases) = bases_node {
            for base in bases.named_children(&mut bases.walk()) {
                attributes.push(Attribute { name: "extends".into(), arguments: vec![node_text(base, content).into()] });
            }
        }

        // Decorators
        attributes.extend(self.extract_decorators(node, content));

        // Docstring
        let documentation = self.extract_docstring(node, content);

        let visibility = if name.starts_with("__") && !name.ends_with("__") {
            Visibility::Private
        } else if name.starts_with("_") {
            Visibility::Protected
        } else {
            Visibility::Public
        };

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Class,
            language: Language::Python,
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

    fn parse_parameters(&self, node: &Node, content: &str) -> Vec<Parameter> {
        let mut params = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            match child.kind() {
                "identifier" => {
                    // Simple parameter: def foo(a)
                    params.push(Parameter {
                        name: node_text(child, content).to_string(),
                        type_ref: None,
                        default_value: None,
                        is_optional: false,
                        is_rest: false,
                    });
                }
                "typed_parameter" => {
                    // Typed parameter: def foo(a: int)
                    let name_node = child.child_by_field_name("name");
                    let type_node = child.child_by_field_name("type");
                    let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                    let type_ref = type_node.and_then(|n| self.parse_type_annotation(&n, content));
                    params.push(Parameter { name, type_ref, default_value: None, is_optional: false, is_rest: false });
                }
                "default_parameter" => {
                    // Parameter with default: def foo(a=1)
                    let name_node = child.child_by_field_name("name");
                    let value_node = child.child_by_field_name("value");
                    let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                    let default_value = value_node.map(|n| node_text(n, content).to_string());
                    params.push(Parameter { name, type_ref: None, default_value, is_optional: true, is_rest: false });
                }
                "typed_default_parameter" => {
                    // Typed with default: def foo(a: int = 1)
                    let name_node = child.child_by_field_name("name");
                    let type_node = child.child_by_field_name("type");
                    let value_node = child.child_by_field_name("value");
                    let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                    let type_ref = type_node.and_then(|n| self.parse_type_annotation(&n, content));
                    let default_value = value_node.map(|n| node_text(n, content).to_string());
                    params.push(Parameter { name, type_ref, default_value, is_optional: true, is_rest: false });
                }
                "list_splat_pattern" => {
                    // *args
                    let name_node = child.child_by_field_name("name");
                    let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                    params.push(Parameter { name, type_ref: None, default_value: None, is_optional: false, is_rest: true });
                }
                "dict_splat_pattern" => {
                    // **kwargs
                    let name_node = child.child_by_field_name("name");
                    let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                    params.push(Parameter { name, type_ref: None, default_value: None, is_optional: false, is_rest: true });
                }
                _ => {}
            }
        }
        params
    }

    fn parse_type_parameters(&self, node: &Node, content: &str) -> Vec<TypeParam> {
        // PEP 695 type parameters
        let mut params = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "type_parameter" {
                let name_node = child.child_by_field_name("name");
                let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                params.push(TypeParam { name, constraint: None, default: None });
            }
        }
        params
    }

    fn parse_type_annotation(&self, node: &Node, content: &str) -> Option<TypeRef> {
        let text = node_text(*node, content).trim_start_matches(':').trim().to_string();
        if text.is_empty() { return None; }
        
        Some(TypeRef {
            name: text,
            generic_args: vec![],
            is_nullable: false,
            is_array: false,
            symbol_id: None,
        })
    }

    fn has_yield(&self, node: &Node) -> bool {
        let mut cursor = node.walk();
        if cursor.goto_first_child() {
            loop {
                if cursor.node().kind() == "yield" || cursor.node().kind() == "yield_from" {
                    return true;
                }
                if !cursor.goto_next_sibling() { break; }
            }
        }
        false
    }

    fn extract_docstring(&self, node: &Node, content: &str) -> Option<String> {
        // Look for string literal as first statement in body
        let body = node.child_by_field_name("body");
        if let Some(body) = body {
            for child in body.named_children(&mut body.walk()) {
                if child.kind() == "expression_statement" {
                    let expr = child.child(0);
                    if let Some(expr) = expr {
                        if expr.kind() == "string" {
                            let text = node_text(expr, content);
                            // Remove quotes
                            return Some(text.trim_matches(|c| c == '"' || c == '\'').to_string());
                        }
                    }
                } else {
                    break; // First non-expression statement
                }
            }
        }
        None
    }

    fn extract_decorators(&self, node: &Node, content: &str) -> Vec<Attribute> {
        let mut attrs = Vec::new();
        // Check for decorated_definition parent
        let mut cursor = node.walk();
        if cursor.goto_parent() {
            if cursor.node().kind() == "decorated_definition" {
                for child in cursor.node().named_children(&mut cursor.node().walk()) {
                    if child.kind() == "decorator" {
                        let name = node_text(child, content).trim_start_matches('@').to_string();
                        attrs.push(Attribute { name, arguments: vec![] });
                    }
                }
            }
        }
        attrs
    }

    fn parse_assignments(&self, node: &Node, content: &str, file: &Path, symbols: &mut Vec<Symbol>) -> Result<()> {
        // Handle module-level assignments: VAR = value
        let left = node.child_by_field_name("left");
        let right = node.child_by_field_name("right");
        
        if let Some(left) = left {
            if left.kind() == "identifier" {
                let name = node_text(left, content).to_string();
                let span = node_to_span(*node, content);
                let is_const = name.chars().all(|c| c.is_uppercase() || c == '_');
                let kind = if is_const { SymbolKind::Constant } else { SymbolKind::Variable };
                let visibility = if name.starts_with("_") { Visibility::Protected } else { Visibility::Public };
                let id = SymbolId::new(Language::Python, &file.to_path_buf(), &name, kind, span);
                
                let type_ref = right.and_then(|n| self.infer_type_from_value(&n, content));
                
                symbols.push(Symbol {
                    id,
                    name,
                    kind,
                    language: Language::Python,
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
                    documentation: None,
                    attributes: Vec::new(),
                    parent: None,
                    children: Vec::new(),
                });
            } else if left.kind() == "pattern_list" || left.kind() == "tuple_pattern" {
                // Multiple assignment: a, b = 1, 2
                for child in left.named_children(&mut left.walk()) {
                    if child.kind() == "identifier" {
                        let name = node_text(child, content).to_string();
                        let span = node_to_span(child, content);
                        let id = SymbolId::new(Language::Python, &file.to_path_buf(), &name, SymbolKind::Variable, span);
                        symbols.push(Symbol {
                            id,
                            name,
                            kind: SymbolKind::Variable,
                            language: Language::Python,
                            file: file.to_path_buf(),
                            span,
                            signature: None,
                            visibility: Visibility::Public,
                            documentation: None,
                            attributes: Vec::new(),
                            parent: None,
                            children: Vec::new(),
                        });
                    }
                }
            }
        }
        Ok(())
    }

    fn infer_type_from_value(&self, node: &Node, _content: &str) -> Option<TypeRef> {
        match node.kind() {
            "string" => Some(TypeRef { name: "str".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "integer" | "float" => Some(TypeRef { name: "int".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "true" | "false" => Some(TypeRef { name: "bool".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "list" | "list_comprehension" => Some(TypeRef { name: "list".into(), generic_args: vec![], is_nullable: false, is_array: true, symbol_id: None }),
            "dictionary" | "dict_comprehension" => Some(TypeRef { name: "dict".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "set" | "set_comprehension" => Some(TypeRef { name: "set".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "tuple" => Some(TypeRef { name: "tuple".into(), generic_args: vec![], is_nullable: false, is_array: false, symbol_id: None }),
            "none" => Some(TypeRef { name: "None".into(), generic_args: vec![], is_nullable: true, is_array: false, symbol_id: None }),
            _ => None,
        }
    }

    fn extract_imports_recursive(&self, node: &Node, content: &str, file: &Path, imports: &mut Vec<Import>) -> Result<()> {
        match node.kind() {
            "import_statement" => {
                if let Some(imp) = self.parse_import(node, content, file)? {
                    imports.push(imp);
                }
            }
            "import_from_statement" => {
                if let Some(imp) = self.parse_import_from(node, content, file)? {
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

    fn parse_import(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Import>> {
        let span = node_to_span(*node, content);
        let mut items = Vec::new();

        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "dotted_name" || child.kind() == "identifier" {
                let name = node_text(child, content).to_string();
                // Check for alias
                let mut cursor = child.walk();
                if cursor.goto_next_sibling() {
                    if cursor.node().kind() == "alias" {
                        let alias_node = cursor.node().child_by_field_name("name");
                        if let Some(alias_node) = alias_node {
                            let alias = node_text(alias_node, content).to_string();
                            items.push(ImportItem { name, alias: Some(alias), symbol_id: None });
                        } else {
                            items.push(ImportItem { name, alias: None, symbol_id: None });
                        }
                    } else {
                        items.push(ImportItem { name, alias: None, symbol_id: None });
                    }
                } else {
                    items.push(ImportItem { name, alias: None, symbol_id: None });
                }
            }
        }

        if items.is_empty() { return Ok(None); }

        Ok(Some(Import {
            file: file.to_path_buf(),
            span,
            module: items[0].name.clone(), // First item is the module
            kind: ImportKind::Static,
            items,
            is_type_only: false,
        }))
    }

    fn parse_import_from(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Import>> {
        let span = node_to_span(*node, content);
        let module_node = node.child_by_field_name("module_name");
        let module = module_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
        
        let mut items = Vec::new();
        
        // Handle: from module import name, name as alias
        let import_list = node.child_by_field_name("name");
        if let Some(list) = import_list {
            for child in list.named_children(&mut list.walk()) {
                if child.kind() == "identifier" || child.kind() == "dotted_name" {
                    let name = node_text(child, content).to_string();
                    let mut cursor = child.walk();
                    if cursor.goto_next_sibling() {
                        if cursor.node().kind() == "alias" {
                            let alias_node = cursor.node().child_by_field_name("name");
                            let alias = alias_node.map(|n| node_text(n, content).to_string());
                            items.push(ImportItem { name, alias, symbol_id: None });
                        } else {
                            items.push(ImportItem { name, alias: None, symbol_id: None });
                        }
                    } else {
                        items.push(ImportItem { name, alias: None, symbol_id: None });
                    }
                } else if child.kind() == "wildcard_import" {
                    items.push(ImportItem { name: "*".into(), alias: None, symbol_id: None });
                }
            }
        }

        if items.is_empty() { return Ok(None); }

        Ok(Some(Import {
            file: file.to_path_buf(),
            span,
            module,
            kind: ImportKind::Static,
            items,
            is_type_only: false,
        }))
    }

    fn extract_exports_recursive(&self, node: &Node, content: &str, file: &Path, exports: &mut Vec<Export>) -> Result<()> {
        // Check for __all__ assignment
        if node.kind() == "assignment" {
            let left = node.child_by_field_name("left");
            if let Some(left) = left {
                if left.kind() == "identifier" && node_text(left, content) == "__all__" {
                    let right = node.child_by_field_name("right");
                    if let Some(right) = right {
                        if right.kind() == "list" || right.kind() == "tuple" {
                            let span = node_to_span(*node, content);
                            let mut items = Vec::new();
                            for child in right.named_children(&mut right.walk()) {
                                if child.kind() == "string" {
                                    let name = node_text(child, content).trim_matches(|c| c == '"' || c == '\'').to_string();
                                    items.push(ExportItem { name, alias: None, symbol_id: None });
                                }
                            }
                            if !items.is_empty() {
                                exports.push(Export {
                                    file: file.to_path_buf(),
                                    span,
                                    kind: ExportKind::Named,
                                    items,
                                });
                            }
                        }
                    }
                }
            }
        }

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

impl Default for PythonParser {
    fn default() -> Self { Self::new() }
}