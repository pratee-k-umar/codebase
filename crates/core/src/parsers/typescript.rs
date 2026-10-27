//! TypeScript/JavaScript parser using tree-sitter.

use tree_sitter::{Node, Tree};
use crate::{Symbol, Import, Export, SymbolId, SymbolKind, Visibility, Signature, Parameter, TypeRef, TypeParam, Attribute, ImportKind, ImportItem, ExportKind, ExportItem, Result, Language, node_to_span, node_text, LanguageParser};
use std::path::Path;

pub struct TypeScriptParser;

impl TypeScriptParser {
    pub fn new() -> Self { Self }
}

impl LanguageParser for TypeScriptParser {
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
        let mut exports = Vec::new();
        let root = tree.root_node();
        self.extract_exports_recursive(&root, content, file, &mut exports)?;
        Ok(exports)
    }

    fn resolve_type(&self, _node: Node, _content: &str, _scope: &crate::parser::Scope) -> Result<Option<TypeRef>> {
        // TODO: Implement type resolution for TypeScript
        Ok(None)
    }
}

impl TypeScriptParser {
    fn extract_symbols_recursive(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        let mut cursor = node.walk();
        let mut children = Vec::new();
        
        // Collect children first (pre-order)
        if cursor.goto_first_child() {
            loop {
                children.push(cursor.node());
                if !cursor.goto_next_sibling() { break; }
            }
        }

        for child in children {
            match child.kind() {
                "function_declaration" => {
                    if let Some(sym) = self.parse_function(&child, content, file, parent.clone(), false)? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "method_definition" => {
                    if let Some(sym) = self.parse_method(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "class_declaration" => {
                    if let Some(sym) = self.parse_class(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "interface_declaration" => {
                    if let Some(sym) = self.parse_interface(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "type_alias_declaration" => {
                    if let Some(sym) = self.parse_type_alias(&child, content, file, parent.clone())? {
                        symbols.push(sym);
                    }
                }
                "enum_declaration" => {
                    if let Some(sym) = self.parse_enum(&child, content, file, parent.clone())? {
                        let sym_id = sym.id.clone();
                        symbols.push(sym);
                        self.extract_symbols_recursive(&child, content, file, Some(sym_id), symbols)?;
                    }
                }
                "variable_declaration" => {
                    self.parse_variables(&child, content, file, parent.clone(), symbols)?;
                }
                "export_statement" | "export_default_declaration" => {
                    // Exports handled separately
                    self.extract_symbols_recursive(&child, content, file, parent.clone(), symbols)?;
                }
                _ => {
                    self.extract_symbols_recursive(&child, content, file, parent.clone(), symbols)?;
                }
            }
        }

        Ok(())
    }

    fn parse_function(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, is_method: bool) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let kind = if is_method { SymbolKind::Method } else { SymbolKind::Function };
        let id = SymbolId::new(Language::TypeScript, &file.to_path_buf(), &name, kind, span);

        // Parse parameters
        let params_node = node.child_by_field_name("parameters");
        let parameters = params_node.map(|n| self.parse_parameters(&n, content)).unwrap_or_default();

        // Parse return type
        let return_type_node = node.child_by_field_name("return_type");
        let return_type = return_type_node.and_then(|n| self.parse_type_annotation(&n, content));

        // Parse type parameters
        let type_params_node = node.child_by_field_name("type_parameters");
        let type_parameters = type_params_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        // Check for async
        let is_async = node.child_by_field_name("async").is_some();
        
        // Check for generator
        let is_generator = node.child_by_field_name("generator").is_some();

        // Check visibility (for methods in classes)
        let visibility = self.determine_visibility(node, content);

        Ok(Some(Symbol {
            id,
            name,
            kind,
            language: Language::TypeScript,
            file: file.to_path_buf(),
            span,
            signature: Some(Signature {
                parameters,
                return_type,
                type_parameters,
                is_async,
                is_generator,
            }),
            visibility,
            documentation: self.extract_jsdoc(node, content),
            attributes: self.extract_decorators(node, content),
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_method(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        self.parse_function(node, content, file, parent, true)
    }

    fn parse_class(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::TypeScript, &file.to_path_buf(), &name, SymbolKind::Class, span);

        // Parse type parameters
        let type_params_node = node.child_by_field_name("type_parameters");
        let _type_parameters = type_params_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        // Parse extends/implements
        let extends = node.child_by_field_name("superclass");
        let implements = node.child_by_field_name("interfaces");

        let mut attributes = self.extract_decorators(node, content);
        if let Some(ext) = extends {
            attributes.push(Attribute { name: "extends".into(), arguments: vec![node_text(ext, content).into()] });
        }
        if let Some(impls) = implements {
            attributes.push(Attribute { name: "implements".into(), arguments: vec![node_text(impls, content).into()] });
        }

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Class,
            language: Language::TypeScript,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility: self.determine_visibility(node, content),
            documentation: self.extract_jsdoc(node, content),
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_interface(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::TypeScript, &file.to_path_buf(), &name, SymbolKind::Interface, span);

        let type_params_node = node.child_by_field_name("type_parameters");
        let _type_parameters = type_params_node.map(|n| self.parse_type_parameters(&n, content)).unwrap_or_default();

        let extends = node.child_by_field_name("extends");
        let mut attributes = Vec::new();
        if let Some(ext) = extends {
            attributes.push(Attribute { name: "extends".into(), arguments: vec![node_text(ext, content).into()] });
        }

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Interface,
            language: Language::TypeScript,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility: Visibility::Public,
            documentation: self.extract_jsdoc(node, content),
            attributes,
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_type_alias(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::TypeScript, &file.to_path_buf(), &name, SymbolKind::TypeAlias, span);

        let value_node = node.child_by_field_name("value");
        let type_ref = value_node.and_then(|n| self.parse_type_annotation(&n, content));

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::TypeAlias,
            language: Language::TypeScript,
            file: file.to_path_buf(),
            span,
            signature: type_ref.map(|t| Signature {
                parameters: vec![],
                return_type: Some(t),
                type_parameters: vec![],
                is_async: false,
                is_generator: false,
            }),
            visibility: Visibility::Public,
            documentation: self.extract_jsdoc(node, content),
            attributes: Vec::new(),
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_enum(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>) -> Result<Option<Symbol>> {
        let name_node = node.child_by_field_name("name");
        let name = name_node.map(|n| node_text(n, content)).unwrap_or("<anonymous>").to_string();
        
        let span = node_to_span(*node, content);
        let id = SymbolId::new(Language::TypeScript, &file.to_path_buf(), &name, SymbolKind::Enum, span);

        Ok(Some(Symbol {
            id,
            name,
            kind: SymbolKind::Enum,
            language: Language::TypeScript,
            file: file.to_path_buf(),
            span,
            signature: None,
            visibility: Visibility::Public,
            documentation: self.extract_jsdoc(node, content),
            attributes: Vec::new(),
            parent,
            children: Vec::new(),
        }))
    }

    fn parse_variables(&self, node: &Node, content: &str, file: &Path, parent: Option<SymbolId>, symbols: &mut Vec<Symbol>) -> Result<()> {
        // Handle const/let/var declarations
        let kind_node = node.child_by_field_name("kind");
        let is_const = kind_node.map(|n| node_text(n, content) == "const").unwrap_or(false);
        
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "variable_declarator" {
                let name_node = child.child_by_field_name("name");
                if let Some(name_node) = name_node {
                    let name = node_text(name_node, content).to_string();
                    let span = node_to_span(child, content);
                    let kind = if is_const { SymbolKind::Constant } else { SymbolKind::Variable };
                    let id = SymbolId::new(Language::TypeScript, &file.to_path_buf(), &name, kind, span);

                    let _value_node = child.child_by_field_name("value");
                    let type_node = child.child_by_field_name("type");
                    let type_ref = type_node.and_then(|n| self.parse_type_annotation(&n, content));

                    symbols.push(Symbol {
                        id,
                        name,
                        kind,
                        language: Language::TypeScript,
                        file: file.to_path_buf(),
                        span,
                        signature: type_ref.map(|t| Signature {
                            parameters: vec![],
                            return_type: Some(t),
                            type_parameters: vec![],
                            is_async: false,
                            is_generator: false,
                        }),
                        visibility: self.determine_visibility(node, content),
                        documentation: self.extract_jsdoc(node, content),
                        attributes: Vec::new(),
                        parent: parent.clone(),
                        children: Vec::new(),
                    });
                }
            }
        }
        Ok(())
    }

    fn parse_parameters(&self, node: &Node, content: &str) -> Vec<Parameter> {
        let mut params = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "required_parameter" || child.kind() == "optional_parameter" || child.kind() == "rest_parameter" {
                let name_node = child.child_by_field_name("name");
                let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                
                let type_node = child.child_by_field_name("type");
                let type_ref = type_node.and_then(|n| self.parse_type_annotation(&n, content));
                
                let is_optional = child.kind() == "optional_parameter";
                let is_rest = child.kind() == "rest_parameter";
                
                let default_node = child.child_by_field_name("value");
                let default_value = default_node.map(|n| node_text(n, content).to_string());
                
                params.push(Parameter {
                    name,
                    type_ref,
                    default_value,
                    is_optional,
                    is_rest,
                });
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
                let constraint = constraint_node.and_then(|n| self.parse_type_annotation(&n, content));
                
                let default_node = child.child_by_field_name("default");
                let default = default_node.and_then(|n| self.parse_type_annotation(&n, content));
                
                params.push(TypeParam { name, constraint, default });
            }
        }
        params
    }

    fn parse_type_annotation(&self, node: &Node, content: &str) -> Option<TypeRef> {
        // Simplified - in reality would handle unions, intersections, generics, etc.
        let text = node_text(*node, content).trim_start_matches(':').trim().to_string();
        if text.is_empty() { return None; }
        
        // Handle array types
        let is_array = text.ends_with("[]");
        let base_type = text.trim_end_matches("[]").trim_end_matches("| null").trim_end_matches("| undefined").to_string();
        let is_nullable = text.contains("| null") || text.contains("| undefined") || text.ends_with("?");
        
        Some(TypeRef {
            name: base_type,
            generic_args: vec![],
            is_nullable,
            is_array,
            symbol_id: None,
        })
    }

    fn determine_visibility(&self, node: &Node, content: &str) -> Visibility {
        // Check for private/public/protected keywords
        for child in node.named_children(&mut node.walk()) {
            match child.kind() {
                "private" => return Visibility::Private,
                "protected" => return Visibility::Protected,
                "public" => return Visibility::Public,
                _ => {}
            }
        }
        // Check for #private fields
        if node_text(*node, content).contains("#") {
            return Visibility::Private;
        }
        Visibility::Public // Default for TS
    }

    fn extract_jsdoc(&self, node: &Node, content: &str) -> Option<String> {
        // Look for preceding JSDoc comment
        let mut cursor = node.walk();
        if cursor.goto_previous_sibling() {
            if cursor.node().kind() == "comment" {
                let text = node_text(cursor.node(), content);
                if text.starts_with("/**") {
                    return Some(text.trim_start_matches("/**").trim_end_matches("*/").trim().to_string());
                }
            }
        }
        None
    }

    fn extract_decorators(&self, node: &Node, content: &str) -> Vec<Attribute> {
        let mut attrs = Vec::new();
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "decorator" {
                let expr = child.child_by_field_name("expression");
                if let Some(expr) = expr {
                    attrs.push(Attribute {
                        name: node_text(expr, content).to_string(),
                        arguments: vec![],
                    });
                }
            }
        }
        attrs
    }

    fn extract_imports_recursive(&self, node: &Node, content: &str, file: &Path, imports: &mut Vec<Import>) -> Result<()> {
        if node.kind() == "import_statement" {
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
        let module_node = node.child_by_field_name("source");
        let module = module_node.map(|n| node_text(n, content).trim_matches(['"', '\''])).unwrap_or("").to_string();
        
        if module.is_empty() { return Ok(None); }

        let span = node_to_span(*node, content);
        let mut items = Vec::new();
        let mut is_type_only = false;

        // Check for "import type"
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "type" {
                is_type_only = true;
            }
        }

        // Parse import clause
        let clause = node.child_by_field_name("import_clause");
        if let Some(clause) = clause {
            for child in clause.named_children(&mut clause.walk()) {
                match child.kind() {
                    "named_imports" => {
                        for spec in child.named_children(&mut child.walk()) {
                            if spec.kind() == "import_specifier" {
                                let name_node = spec.child_by_field_name("name");
                                let alias_node = spec.child_by_field_name("alias");
                                let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                                let alias = alias_node.map(|n| node_text(n, content)).map(|s| s.to_string());
                                if !name.is_empty() {
                                    items.push(ImportItem { name, alias, symbol_id: None });
                                }
                            }
                        }
                    }
                    "identifier" => {
                        // Default import
                        let name = node_text(child, content).to_string();
                        items.push(ImportItem { name, alias: None, symbol_id: None });
                    }
                    "namespace_import" => {
                        // import * as name
                        let name_node = child.child_by_field_name("name");
                        if let Some(n) = name_node {
                            let name = node_text(n, content).to_string();
                            items.push(ImportItem { name, alias: None, symbol_id: None });
                        }
                    }
                    _ => {}
                }
            }
        }

        Ok(Some(Import {
            file: file.to_path_buf(),
            span,
            module,
            kind: ImportKind::Static,
            items,
            is_type_only,
        }))
    }

    fn extract_exports_recursive(&self, node: &Node, content: &str, file: &Path, exports: &mut Vec<Export>) -> Result<()> {
        match node.kind() {
            "export_statement" => {
                if let Some(exp) = self.parse_export(node, content, file)? {
                    exports.push(exp);
                }
            }
            "export_default_declaration" => {
                if let Some(exp) = self.parse_default_export(node, content, file)? {
                    exports.push(exp);
                }
            }
            _ => {}
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

    fn parse_export(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Export>> {
        let span = node_to_span(*node, content);
        let mut items = Vec::new();
        let mut kind = ExportKind::Named;

        // Check for "export type"
        let mut is_type = false;
        for child in node.named_children(&mut node.walk()) {
            if child.kind() == "type" {
                is_type = true;
            }
        }

        let declaration = node.child_by_field_name("declaration");
        let source = node.child_by_field_name("source");

        if let Some(source) = source {
            // Re-export: export { ... } from "..."
            kind = ExportKind::ReExport;
            let _module = node_text(source, content).trim_matches(['"', '\'']).to_string();
            
            let clause = node.child_by_field_name("export_clause");
            if let Some(clause) = clause {
                for spec in clause.named_children(&mut clause.walk()) {
                    if spec.kind() == "export_specifier" {
                        let name_node = spec.child_by_field_name("name");
                        let alias_node = spec.child_by_field_name("alias");
                        let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                        let alias = alias_node.map(|n| node_text(n, content)).map(|s| s.to_string());
                        if !name.is_empty() {
                            items.push(ExportItem { name, alias, symbol_id: None });
                        }
                    }
                }
            }
            
            if is_type {
                kind = ExportKind::Named; // Type exports are still named exports
            }
        } else if let Some(decl) = declaration {
            // Direct export: export function foo() {}
            match decl.kind() {
                "function_declaration" => {
                    let name_node = decl.child_by_field_name("name");
                    if let Some(n) = name_node {
                        let name = node_text(n, content).to_string();
                        items.push(ExportItem { name, alias: None, symbol_id: None });
                    }
                }
                "class_declaration" => {
                    let name_node = decl.child_by_field_name("name");
                    if let Some(n) = name_node {
                        let name = node_text(n, content).to_string();
                        items.push(ExportItem { name, alias: None, symbol_id: None });
                    }
                }
                "interface_declaration" => {
                    let name_node = decl.child_by_field_name("name");
                    if let Some(n) = name_node {
                        let name = node_text(n, content).to_string();
                        items.push(ExportItem { name, alias: None, symbol_id: None });
                    }
                }
                "type_alias_declaration" => {
                    let name_node = decl.child_by_field_name("name");
                    if let Some(n) = name_node {
                        let name = node_text(n, content).to_string();
                        items.push(ExportItem { name, alias: None, symbol_id: None });
                    }
                }
                "enum_declaration" => {
                    let name_node = decl.child_by_field_name("name");
                    if let Some(n) = name_node {
                        let name = node_text(n, content).to_string();
                        items.push(ExportItem { name, alias: None, symbol_id: None });
                    }
                }
                _ => {}
            }
        } else {
            // export { a, b, c }
            let clause = node.child_by_field_name("export_clause");
            if let Some(clause) = clause {
                for spec in clause.named_children(&mut clause.walk()) {
                    if spec.kind() == "export_specifier" {
                        let name_node = spec.child_by_field_name("name");
                        let alias_node = spec.child_by_field_name("alias");
                        let name = name_node.map(|n| node_text(n, content)).unwrap_or("").to_string();
                        let alias = alias_node.map(|n| node_text(n, content)).map(|s| s.to_string());
                        if !name.is_empty() {
                            items.push(ExportItem { name, alias, symbol_id: None });
                        }
                    }
                }
            }
        }

        if items.is_empty() { return Ok(None); }

        Ok(Some(Export {
            file: file.to_path_buf(),
            span,
            kind,
            items,
        }))
    }

    fn parse_default_export(&self, node: &Node, content: &str, file: &Path) -> Result<Option<Export>> {
        let span = node_to_span(*node, content);
        let declaration = node.child_by_field_name("declaration");
        
        let mut items = Vec::new();
        if let Some(decl) = declaration {
            // Anonymous default export - give it a synthetic name
            let name = match decl.kind() {
                "function_declaration" | "class_declaration" => "default".into(),
                _ => "default".into(),
            };
            items.push(ExportItem { name, alias: None, symbol_id: None });
        } else {
            // export default expression
            items.push(ExportItem { name: "default".into(), alias: None, symbol_id: None });
        }

        Ok(Some(Export {
            file: file.to_path_buf(),
            span,
            kind: ExportKind::Default,
            items,
        }))
    }
}

impl Default for TypeScriptParser {
    fn default() -> Self { Self::new() }
}