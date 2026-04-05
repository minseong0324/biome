use biome_analyze::{Rule, RuleDiagnostic, RuleDomain, context::RuleContext, declare_lint_rule};
use biome_console::markup;
use biome_js_syntax::{
    AnyJsExpression, AnyJsFunction, AnyJsFunctionBody, JsFunctionBody, JsReturnStatement,
    JsSyntaxKind, JsSyntaxNode, JsVariableDeclarator, JsVariableStatement, TsAsExpression,
    TsTypeAssertionExpression,
};
use biome_js_type_info::{Literal, Type, TypeData};
use biome_rowan::{AstNode, TextRange};
use biome_rule_options::no_misleading_return_type::NoMisleadingReturnTypeOptions;

use crate::services::typed::Typed;

declare_lint_rule! {
    /// Detect return type annotations that are misleadingly wider than what
    /// the implementation actually returns.
    ///
    /// Reports when a function's explicit return type annotation is wider than
    /// what TypeScript would infer from the implementation, hiding precise types
    /// from callers.
    ///
    /// ## Examples
    ///
    /// ### Invalid
    ///
    /// ```ts,expect_diagnostic
    /// function getStatus(b: boolean): string { if (b) return "loading"; return "idle"; }
    /// ```
    ///
    /// ```ts,expect_diagnostic
    /// function getCode(ok: boolean): number { if (ok) return 200; return 404; }
    /// ```
    ///
    /// ### Valid
    ///
    /// ```ts
    /// function getStatus() { return "loading"; }
    /// ```
    ///
    /// ```ts
    /// function run(): void { return; }
    /// ```
    pub NoMisleadingReturnType {
        version: "next",
        name: "noMisleadingReturnType",
        language: "ts",
        recommended: false,
        domains: &[RuleDomain::Types],
    }
}

pub struct RuleState {
    annotation_range: TextRange,
    inferred_description: String,
}

impl Rule for NoMisleadingReturnType {
    type Query = Typed<AnyJsFunction>;
    type State = RuleState;
    type Signals = Option<Self::State>;
    type Options = NoMisleadingReturnTypeOptions;

    fn run(ctx: &RuleContext<Self>) -> Self::Signals {
        let node = ctx.query();

        let annotation = node.return_type_annotation()?;
        let annotation_range = annotation.range();

        if node.is_generator() || is_overload_implementation(node) {
            return None;
        }

        let func_type = ctx.type_of_function(node);
        let return_ty = extract_return_type(&func_type)?;

        if is_escape_hatch(&return_ty) {
            return None;
        }

        let effective_return_ty = if node.async_token().is_some() {
            unwrap_promise_inner(&return_ty)
        } else {
            return_ty.clone()
        };

        let body = node.body().ok()?;
        let (returns, has_any_const_return) = collect_return_info(ctx, &body);

        if returns.is_empty() {
            return None;
        }

        if returns.len() == 1 && !has_any_const_return && is_literal_of_primitive(&returns[0]) {
            return None;
        }

        if matches!(&*effective_return_ty, TypeData::Boolean)
            && returns.iter().any(|ty| matches!(&**ty, TypeData::Literal(lit) if matches!(lit.as_ref(), Literal::Boolean(b) if b.as_bool())))
            && returns.iter().any(|ty| matches!(&**ty, TypeData::Literal(lit) if matches!(lit.as_ref(), Literal::Boolean(b) if !b.as_bool())))
        {
            return None;
        }

        if returns.iter().any(is_any_contaminated) {
            return None;
        }

        if includes_undefined(&effective_return_ty)
            && !returns.iter().any(includes_undefined)
        {
            return None;
        }

        if returns.iter().any(is_intersection_with_type_param) {
            return None;
        }

        if !has_any_const_return
            && is_only_property_literal_widening(&effective_return_ty, &returns, 0)
        {
            return None;
        }

        let is_misleading = if effective_return_ty.is_union() {
            is_union_wider_than_returns(&effective_return_ty, &returns)
        } else {
            returns
                .iter()
                .all(|inferred| is_wider_than(&effective_return_ty, inferred, 0))
        };

        if !is_misleading {
            return None;
        }

        let inferred_description = build_inferred_description(&returns, &effective_return_ty);
        Some(RuleState {
            annotation_range,
            inferred_description,
        })
    }

    fn diagnostic(_ctx: &RuleContext<Self>, state: &Self::State) -> Option<RuleDiagnostic> {
        let diag = RuleDiagnostic::new(
            rule_category!(),
            state.annotation_range,
            markup! {
                "The return type annotation is wider than what the function actually returns."
            },
        )
        .note(markup! {
            "A wider return type hides the precise types that callers could rely on."
        });

        let diag = if state.inferred_description.is_empty() {
            diag
        } else {
            let desc = state.inferred_description.as_str();
            diag.note(markup! {
                "The inferred return type is narrower: "{desc}"."
            })
        };

        Some(diag)
    }

}

fn is_overload_implementation(node: &AnyJsFunction) -> bool {
    let name = node
        .binding()
        .and_then(|b| b.as_js_identifier_binding().cloned())
        .and_then(|id| id.name_token().ok())
        .map(|t| t.text_trimmed().to_string());
    let Some(name) = name else { return false };

    let Some(parent) = node.syntax().parent() else {
        return false;
    };
    parent.children().any(|sibling| {
        if sibling == *node.syntax() {
            return false;
        }
        if let Some(decl) =
            biome_js_syntax::TsDeclareFunctionDeclaration::cast(sibling.clone())
        {
            return decl
                .id()
                .ok()
                .and_then(|id| id.as_js_identifier_binding().cloned())
                .and_then(|id| id.name_token().ok())
                .is_some_and(|t| t.text_trimmed() == name);
        }
        AnyJsFunction::cast(sibling).is_some_and(|sib_fn| {
            sib_fn.body().is_err()
                && sib_fn
                    .binding()
                    .and_then(|b| b.as_js_identifier_binding().cloned())
                    .and_then(|id| id.name_token().ok())
                    .is_some_and(|t| t.text_trimmed() == name)
        })
    })
}

fn extract_return_type(func_type: &Type) -> Option<Type> {
    match &**func_type {
        TypeData::Function(function) => {
            let ty_ref = function.return_type.as_type()?;
            func_type.resolve(ty_ref)
        }
        _ => None,
    }
}

fn is_escape_hatch(ty: &Type) -> bool {
    matches!(
        &**ty,
        TypeData::AnyKeyword
            | TypeData::VoidKeyword
            | TypeData::UnknownKeyword
            | TypeData::NeverKeyword
            | TypeData::Unknown
            | TypeData::ThisKeyword
    )
}

fn unwrap_promise_inner(return_ty: &Type) -> Type {
    if let TypeData::InstanceOf(instance) = &**return_ty
        && let Some(inner_ref) = instance.type_parameters.first()
            && let Some(inner) = return_ty.resolve(inner_ref)
                && !is_escape_hatch(&inner) {
                    return inner;
                }

    return_ty.clone()
}

fn includes_undefined(ty: &Type) -> bool {
    match &**ty {
        TypeData::Undefined | TypeData::VoidKeyword => true,
        TypeData::Union(_) => ty
            .flattened_union_variants()
            .any(|v| matches!(&*v, TypeData::Undefined | TypeData::VoidKeyword)),
        _ => false,
    }
}

fn is_any_contaminated(ty: &Type) -> bool {
    match &**ty {
        TypeData::AnyKeyword => true,
        TypeData::Union(_) => ty
            .flattened_union_variants()
            .any(|v| matches!(&*v, TypeData::AnyKeyword)),
        _ => false,
    }
}

fn is_intersection_with_type_param(ty: &Type) -> bool {
    match &**ty {
        TypeData::Intersection(intersection) => intersection.types().iter().any(|member_ref| {
            ty.resolve(member_ref)
                .is_some_and(|resolved| matches!(&*resolved, TypeData::Generic(_)))
        }),
        _ => false,
    }
}

fn is_literal_of_primitive(ty: &Type) -> bool {
    matches!(&**ty, TypeData::Literal(lit) if lit.is_primitive())
}

fn is_only_property_literal_widening(annotation: &Type, returns: &[Type], depth: u8) -> bool {
    if depth > 3 {
        return false;
    }

    if let TypeData::Tuple(ann_tuple) = &**annotation {
        return returns.iter().all(|inferred| {
            let TypeData::Tuple(inf_tuple) = &**inferred else {
                return false;
            };
            let ann_elems = ann_tuple.elements();
            let inf_elems = inf_tuple.elements();
            if ann_elems.len() != inf_elems.len() || ann_elems.is_empty() {
                return false;
            }
            let mut has_widening = false;
            for (ann_elem, inf_elem) in ann_elems.iter().zip(inf_elems.iter()) {
                let ann_ty = annotation.resolve(&ann_elem.ty);
                let inf_ty = inferred.resolve(&inf_elem.ty);
                match (ann_ty, inf_ty) {
                    (Some(a), Some(b)) => {
                        if types_match(&a, &b) {
                            continue;
                        }
                        if is_base_type_of_literal(&a, &b) {
                            has_widening = true;
                        } else {
                            return false;
                        }
                    }
                    _ => return false,
                }
            }
            has_widening
        });
    }

    let (TypeData::Object(ann_obj), _) = (&**annotation, ()) else {
        return false;
    };

    if ann_obj.members.is_empty() {
        return false;
    }

    returns.iter().all(|inferred| {
        let inf_members = match &**inferred {
            TypeData::Object(obj) => &obj.members,
            TypeData::Literal(lit) => match lit.as_ref() {
                Literal::Object(obj_lit) => obj_lit.members(),
                _ => return false,
            },
            _ => return false,
        };

        if inf_members.is_empty() {
            return false;
        }

        let mut has_widening = false;

        let ann_index_sig = ann_obj.members.iter().find(|m| {
            matches!(m.kind, biome_js_type_info::TypeMemberKind::IndexSignature(_))
        });
        if let Some(sig_member) = ann_index_sig
            && let Some(sig_value_ty) = annotation.resolve(&sig_member.ty) {
                let mut sig_has_widening = false;
                let all_ok = inf_members.iter().all(|inf_m| {
                    if let Some(inf_ty) = annotation.resolve(&inf_m.ty) {
                        if types_match(&sig_value_ty, &inf_ty) {
                            return true;
                        }
                        if is_base_type_of_literal(&sig_value_ty, &inf_ty) {
                            sig_has_widening = true;
                            return true;
                        }
                    }
                    false
                });
                return all_ok && sig_has_widening;
            }

        for ann_member in ann_obj.members.iter() {
            let ann_name = match &ann_member.kind {
                biome_js_type_info::TypeMemberKind::Named(name) => name,
                _ => continue,
            };

            let inf_member = inf_members.iter().find(|m| m.kind.has_name(ann_name));
            let Some(inf_member) = inf_member else {
                return false;
            };

            let ann_ty = annotation.resolve(&ann_member.ty);
            let inf_ty = annotation.resolve(&inf_member.ty);

            match (ann_ty, inf_ty) {
                (Some(a), Some(b)) => {
                    if types_match(&a, &b) {
                        continue;
                    }
                    if is_base_type_of_literal(&a, &b) {
                        has_widening = true;
                    } else {
                        return false;
                    }
                }
                _ => return false,
            }
        }

        has_widening
    })
}

fn is_base_type_of_literal(base: &Type, literal: &Type) -> bool {
    match (&**base, &**literal) {
        (TypeData::String, TypeData::Literal(lit)) => {
            matches!(lit.as_ref(), Literal::String(_) | Literal::Template(_))
        }
        (TypeData::Number, TypeData::Literal(lit)) => matches!(lit.as_ref(), Literal::Number(_)),
        (TypeData::Boolean, TypeData::Literal(lit)) => {
            matches!(lit.as_ref(), Literal::Boolean(_))
        }
        (TypeData::BigInt, TypeData::Literal(lit)) => matches!(lit.as_ref(), Literal::BigInt(_)),
        _ => false,
    }
}

fn build_inferred_description(returns: &[Type], annotation: &Type) -> String {
    let candidates: Vec<Type> = match &**annotation {
        TypeData::Union(_) => annotation.flattened_union_variants().collect(),
        _ => returns.to_vec(),
    };

    let mut parts: Vec<String> = Vec::new();
    for ty in &candidates {
        match &**ty {
            TypeData::Literal(lit) => match lit.as_ref() {
                Literal::String(s) => {
                    parts.push(format!("\"{}\"", s.as_str()));
                }
                Literal::Number(n) => {
                    parts.push(n.as_str().to_string());
                }
                Literal::Boolean(b) => {
                    parts.push(if b.as_bool() { "true" } else { "false" }.to_string());
                }
                _ => return String::new(),
            },
            _ => return String::new(),
        }
    }

    if parts.is_empty() {
        return String::new();
    }

    let result = parts.join(" | ");

    if result.contains("...") || result.contains("__internal") || result.contains("typeof import(") {
        return String::new();
    }

    if result.len() > 80 {
        return String::new();
    }

    result
}

fn collect_return_info(
    ctx: &RuleContext<NoMisleadingReturnType>,
    body: &AnyJsFunctionBody,
) -> (Vec<Type>, bool) {
    let mut has_any_const = false;

    let types = match body {
        AnyJsFunctionBody::JsFunctionBody(block) => {
            collect_block_returns(ctx, block, &mut has_any_const)
        }
        AnyJsFunctionBody::AnyJsExpression(expr) => {
            if has_const_assertion(expr) {
                has_any_const = true;
            }
            vec![infer_expression_type(ctx, expr)]
        }
    };

    (types, has_any_const)
}

fn collect_block_returns(
    ctx: &RuleContext<NoMisleadingReturnType>,
    block: &JsFunctionBody,
    has_any_const: &mut bool,
) -> Vec<Type> {
    let mut returns = Vec::new();
    let mut stack: Vec<JsSyntaxNode> = vec![block.syntax().clone()];

    while let Some(node) = stack.pop() {
        for child in node.children() {
            if is_nested_function_like(&child) {
                continue;
            }

            if let Some(ret) = JsReturnStatement::cast(child.clone()) {
                if let Some(arg) = ret.argument()
                    && let Some(expr) = AnyJsExpression::cast(arg.syntax().clone()) {
                        if has_const_assertion(&expr) {
                            *has_any_const = true;
                        }
                        returns.push(infer_expression_type(ctx, &expr));
                    }
                continue;
            }

            stack.push(child);
        }
    }

    returns
}

fn infer_expression_type(
    ctx: &RuleContext<NoMisleadingReturnType>,
    expr: &AnyJsExpression,
) -> Type {
    let inner = unwrap_type_wrappers(expr);

    if let AnyJsExpression::JsIdentifierExpression(ref id_expr) = inner
        && let Some(init_type) = resolve_identifier_initializer_type(ctx, id_expr) {
            return init_type;
        }

    ctx.type_of_expression(&inner)
}

fn resolve_identifier_initializer_type(
    ctx: &RuleContext<NoMisleadingReturnType>,
    id_expr: &biome_js_syntax::JsIdentifierExpression,
) -> Option<Type> {
    let name = id_expr
        .name()
        .ok()
        .and_then(|n| n.value_token().ok())
        .map(|t| t.text_trimmed().to_string())?;

    let body_node = id_expr
        .syntax()
        .ancestors()
        .find(|ancestor| ancestor.kind() == JsSyntaxKind::JS_FUNCTION_BODY)?;
    let body = JsFunctionBody::cast(body_node)?;

    for stmt in body.statements() {
        let var_stmt = JsVariableStatement::cast(stmt.into_syntax());
        let Some(var_stmt) = var_stmt else { continue };
        let Ok(decl) = var_stmt.declaration() else {
            continue;
        };
        for declarator in decl.declarators() {
            let Ok(d) = declarator else { continue };
            let id_text = d
                .id()
                .ok()
                .and_then(|id| id.as_any_js_binding().cloned())
                .and_then(|b| b.as_js_identifier_binding().cloned())
                .and_then(|ib| ib.name_token().ok())
                .map(|t| t.text_trimmed().to_string());
            let Some(id_text) = id_text else { continue };
            if id_text != name {
                continue;
            }
            let init_expr = d
                .initializer()
                .and_then(|init| init.expression().ok())?;
            if !has_const_assertion(&init_expr) {
                continue;
            }
            let unwrapped = unwrap_type_wrappers(&init_expr);
            return Some(ctx.type_of_expression(&unwrapped));
        }
    }

    None
}

fn unwrap_type_wrappers(expr: &AnyJsExpression) -> AnyJsExpression {
    match expr {
        AnyJsExpression::TsAsExpression(e) => e
            .expression()
            .map_or_else(|_| expr.clone(), |inner| unwrap_type_wrappers(&inner)),
        AnyJsExpression::TsSatisfiesExpression(e) => e
            .expression()
            .map_or_else(|_| expr.clone(), |inner| unwrap_type_wrappers(&inner)),
        AnyJsExpression::TsTypeAssertionExpression(e) => e
            .expression()
            .map_or_else(|_| expr.clone(), |inner| unwrap_type_wrappers(&inner)),
        AnyJsExpression::JsParenthesizedExpression(e) => e
            .expression()
            .map_or_else(|_| expr.clone(), |inner| unwrap_type_wrappers(&inner)),
        _ => expr.clone(),
    }
}

fn has_const_assertion(expr: &AnyJsExpression) -> bool {
    match expr {
        AnyJsExpression::TsAsExpression(e) => is_const_type_assertion(e),
        AnyJsExpression::TsTypeAssertionExpression(e) => is_const_angle_bracket_assertion(e),
        AnyJsExpression::JsParenthesizedExpression(e) => {
            e.expression().is_ok_and(|inner| has_const_assertion(&inner))
        }
        AnyJsExpression::TsSatisfiesExpression(e) => {
            e.expression().is_ok_and(|inner| has_const_assertion(&inner))
        }
        AnyJsExpression::JsIdentifierExpression(id_expr) => {
            identifier_refers_to_const_assertion(id_expr)
        }
        _ => false,
    }
}

fn identifier_refers_to_const_assertion(
    id_expr: &biome_js_syntax::JsIdentifierExpression,
) -> bool {
    let name = id_expr
        .name()
        .ok()
        .and_then(|n| n.value_token().ok())
        .map(|t| t.text_trimmed().to_string());
    let Some(name) = name else { return false };

    let enclosing_body = id_expr
        .syntax()
        .ancestors()
        .find(|ancestor| ancestor.kind() == JsSyntaxKind::JS_FUNCTION_BODY);
    let Some(body_node) = enclosing_body else {
        return false;
    };
    let Some(body) = JsFunctionBody::cast(body_node) else {
        return false;
    };

    body.statements().into_iter().any(|stmt| {
        let var_stmt = JsVariableStatement::cast(stmt.into_syntax());
        let Some(var_stmt) = var_stmt else { return false };
        let Ok(decl) = var_stmt.declaration() else {
            return false;
        };
        decl.declarators().into_iter().any(|declarator| {
            declarator
                .ok()
                .is_some_and(|d| declarator_matches_name_with_const(&d, &name))
        })
    })
}

fn declarator_matches_name_with_const(declarator: &JsVariableDeclarator, name: &str) -> bool {
    let id_text = declarator
        .id()
        .ok()
        .and_then(|id| id.as_any_js_binding().cloned())
        .and_then(|b| b.as_js_identifier_binding().cloned())
        .and_then(|ib| ib.name_token().ok())
        .map(|t| t.text_trimmed().to_string());
    let Some(id_text) = id_text else { return false };

    if id_text != name {
        return false;
    }

    declarator
        .initializer()
        .and_then(|init| init.expression().ok())
        .is_some_and(|init_expr| has_const_assertion(&init_expr))
}

fn is_const_type_assertion(expr: &TsAsExpression) -> bool {
    is_const_reference_type(&expr.ty().ok())
}

fn is_const_angle_bracket_assertion(expr: &TsTypeAssertionExpression) -> bool {
    is_const_reference_type(&expr.ty().ok())
}

fn is_const_reference_type(ty: &Option<biome_js_syntax::AnyTsType>) -> bool {
    ty.as_ref()
        .and_then(|ty| ty.as_ts_reference_type())
        .and_then(|ref_ty| ref_ty.name().ok())
        .is_some_and(|name| {
            name.as_js_reference_identifier()
                .and_then(|id| id.value_token().ok())
                .is_some_and(|token| token.text_trimmed() == "const")
        })
}

fn is_nested_function_like(node: &JsSyntaxNode) -> bool {
    matches!(
        node.kind(),
        JsSyntaxKind::JS_FUNCTION_EXPRESSION
            | JsSyntaxKind::JS_ARROW_FUNCTION_EXPRESSION
            | JsSyntaxKind::JS_FUNCTION_DECLARATION
            | JsSyntaxKind::JS_CONSTRUCTOR_CLASS_MEMBER
            | JsSyntaxKind::JS_METHOD_CLASS_MEMBER
            | JsSyntaxKind::JS_METHOD_OBJECT_MEMBER
            | JsSyntaxKind::JS_GETTER_CLASS_MEMBER
            | JsSyntaxKind::JS_GETTER_OBJECT_MEMBER
            | JsSyntaxKind::JS_SETTER_CLASS_MEMBER
            | JsSyntaxKind::JS_SETTER_OBJECT_MEMBER
    )
}

fn is_wider_than(annotated: &Type, inferred: &Type, depth: u8) -> bool {
    if depth > 5 {
        return false;
    }

    match (&**annotated, &**inferred) {
        (TypeData::String, TypeData::Literal(lit)) => {
            matches!(lit.as_ref(), Literal::String(_) | Literal::Template(_))
        }
        (TypeData::Number, TypeData::Literal(lit)) => matches!(lit.as_ref(), Literal::Number(_)),
        (TypeData::Boolean, TypeData::Literal(lit)) => {
            matches!(lit.as_ref(), Literal::Boolean(_))
        }
        (TypeData::BigInt, TypeData::Literal(lit)) => matches!(lit.as_ref(), Literal::BigInt(_)),

        (TypeData::String, TypeData::String)
        | (TypeData::Number, TypeData::Number)
        | (TypeData::Boolean, TypeData::Boolean)
        | (TypeData::BigInt, TypeData::BigInt) => false,

        (TypeData::Union(_), _) => is_union_wider(annotated, inferred, depth),

        (TypeData::InstanceOf(ann_inst), TypeData::InstanceOf(inf_inst)) => {
            is_instance_wider(annotated, ann_inst, inferred, inf_inst, depth)
        }

        (TypeData::Object(ann_obj), TypeData::Object(inf_obj)) => {
            is_object_wider(annotated, ann_obj, inferred, inf_obj, depth)
        }
        (TypeData::Object(ann_obj), TypeData::Literal(lit)) => match lit.as_ref() {
            Literal::Object(inf_lit) => {
                is_object_wider_than_literal(annotated, ann_obj, inf_lit, depth)
            }
            _ => false,
        },

        (_, TypeData::Generic(generic)) if generic.constraint.is_known() => {
            if let Some(constraint) = inferred.resolve(&generic.constraint) {
                is_wider_than(annotated, &constraint, depth + 1)
            } else {
                false
            }
        }

        (TypeData::Unknown, _) | (_, TypeData::Unknown) => false,
        _ => false,
    }
}

fn is_union_wider_than_returns(annotated: &Type, returns: &[Type]) -> bool {
    let ann_variants: Vec<Type> = annotated.flattened_union_variants().collect();
    if ann_variants.is_empty() {
        return false;
    }

    let all_covered = returns.iter().all(|ret| {
        ann_variants
            .iter()
            .any(|ann_v| types_match(ann_v, ret) || is_wider_than(ann_v, ret, 0))
    });

    if !all_covered {
        return false;
    }

    ann_variants.iter().any(|ann_v| {
        !returns.iter().any(|ret| {
            types_match(ann_v, ret) || is_wider_than(ann_v, ret, 0)
        })
    })
}

fn is_union_wider(annotated: &Type, inferred: &Type, depth: u8) -> bool {
    let ann_variants: Vec<Type> = annotated.flattened_union_variants().collect();
    if ann_variants.is_empty() {
        return false;
    }

    let inf_variants: Vec<Type> = match &**inferred {
        TypeData::Union(_) => inferred.flattened_union_variants().collect(),
        _ => vec![inferred.clone()],
    };

    if inf_variants.is_empty() {
        return false;
    }

    let all_inferred_covered = inf_variants.iter().all(|inf_v| {
        ann_variants.iter().any(|ann_v| {
            types_match(ann_v, inf_v) || is_wider_than(ann_v, inf_v, depth + 1)
        })
    });

    if !all_inferred_covered {
        return false;
    }

    let effective_ann_variants: Vec<&Type> = ann_variants
        .iter()
        .filter(|ann_v| {
            if let TypeData::Generic(generic) = &***ann_v
                && generic.constraint.is_known()
                    && let Some(constraint) = ann_v.resolve(&generic.constraint) {
                        let subsumed = ann_variants.iter().any(|other| {
                            !std::ptr::eq(*ann_v as *const Type, other as *const Type)
                                && (types_match(other, &constraint)
                                    || is_wider_than(other, &constraint, depth + 1))
                        });
                        return !subsumed;
                    }
            true
        })
        .collect();

    effective_ann_variants.iter().any(|ann_v| {
        !inf_variants.iter().any(|inf_v| {
            types_match(ann_v, inf_v) || is_wider_than(ann_v, inf_v, depth + 1)
        })
    })
}

fn types_match(a: &Type, b: &Type) -> bool {
    match (&**a, &**b) {
        (TypeData::String, TypeData::String)
        | (TypeData::Number, TypeData::Number)
        | (TypeData::Boolean, TypeData::Boolean)
        | (TypeData::BigInt, TypeData::BigInt)
        | (TypeData::Null, TypeData::Null)
        | (TypeData::Undefined, TypeData::Undefined)
        | (TypeData::VoidKeyword, TypeData::VoidKeyword)
        | (TypeData::NeverKeyword, TypeData::NeverKeyword) => true,

        (TypeData::Literal(a_lit), TypeData::Literal(b_lit)) => a_lit == b_lit,

        (TypeData::Generic(a_gen), TypeData::Generic(b_gen)) => a_gen.name == b_gen.name,

        (TypeData::InstanceOf(a_inst), TypeData::InstanceOf(b_inst))
            if a_inst.type_parameters.is_empty() && b_inst.type_parameters.is_empty() =>
        {
            let a_base = a.resolve(&a_inst.ty);
            let b_base = b.resolve(&b_inst.ty);
            match (a_base, b_base) {
                (Some(a_base), Some(b_base)) => types_match(&a_base, &b_base),
                _ => false,
            }
        }

        (TypeData::Generic(a_gen), TypeData::InstanceOf(b_inst))
            if b_inst.type_parameters.is_empty() =>
        {
            if let Some(base) = b.resolve(&b_inst.ty)
                && let TypeData::Generic(b_gen) = &*base {
                    return a_gen.name == b_gen.name;
                }
            false
        }
        (TypeData::InstanceOf(a_inst), TypeData::Generic(b_gen))
            if a_inst.type_parameters.is_empty() =>
        {
            if let Some(base) = a.resolve(&a_inst.ty)
                && let TypeData::Generic(a_gen) = &*base {
                    return a_gen.name == b_gen.name;
                }
            false
        }

        _ => false,
    }
}

fn is_instance_wider(
    annotated: &Type,
    ann_inst: &biome_js_type_info::TypeInstance,
    inferred: &Type,
    inf_inst: &biome_js_type_info::TypeInstance,
    depth: u8,
) -> bool {
    let ann_base = annotated.resolve(&ann_inst.ty);
    let inf_base = inferred.resolve(&inf_inst.ty);
    let same_base = match (&ann_base, &inf_base) {
        (Some(a), Some(b)) => types_match(a, b),
        _ => false,
    };
    if !same_base {
        return false;
    }

    let ann_params = &ann_inst.type_parameters;
    let inf_params = &inf_inst.type_parameters;
    if ann_params.len() != inf_params.len() || ann_params.is_empty() {
        return false;
    }

    ann_params
        .iter()
        .zip(inf_params.iter())
        .any(|(ann_p, inf_p)| match (annotated.resolve(ann_p), inferred.resolve(inf_p)) {
            (Some(a), Some(b)) => is_wider_than(&a, &b, depth + 1),
            _ => false,
        })
}

fn is_object_wider(
    annotated: &Type,
    ann_obj: &biome_js_type_info::Object,
    inferred: &Type,
    inf_obj: &biome_js_type_info::Object,
    depth: u8,
) -> bool {
    if ann_obj.members.is_empty() || inf_obj.members.is_empty() {
        return false;
    }

    let ann_index_sig = ann_obj.members.iter().find(|m| {
        matches!(m.kind, biome_js_type_info::TypeMemberKind::IndexSignature(_))
    });
    if let Some(sig_member) = ann_index_sig
        && let Some(sig_value_ty) = annotated.resolve(&sig_member.ty) {
            let all_narrower = inf_obj.members.iter().all(|inf_m| {
                if let Some(inf_ty) = inferred.resolve(&inf_m.ty) {
                    is_wider_than(&sig_value_ty, &inf_ty, depth + 1) || types_match(&sig_value_ty, &inf_ty)
                } else {
                    false
                }
            });
            let any_wider = inf_obj.members.iter().any(|inf_m| {
                inferred.resolve(&inf_m.ty).is_some_and(|inf_ty| {
                    is_wider_than(&sig_value_ty, &inf_ty, depth + 1)
                })
            });
            if all_narrower && any_wider {
                return true;
            }
        }

    let mut has_any_wider = false;

    for ann_member in ann_obj.members.iter() {
        let ann_name = match &ann_member.kind {
            biome_js_type_info::TypeMemberKind::Named(name) => name,
            _ => continue,
        };

        let inf_member = inf_obj.members.iter().find(|m| m.kind.has_name(ann_name));
        let Some(inf_member) = inf_member else {
            return false;
        };

        let ann_prop_ty = annotated.resolve(&ann_member.ty);
        let inf_prop_ty = inferred.resolve(&inf_member.ty);

        match (ann_prop_ty, inf_prop_ty) {
            (Some(a), Some(b)) => {
                if types_match(&a, &b) {
                    continue;
                }
                if is_wider_than(&a, &b, depth + 1) {
                    has_any_wider = true;
                } else {
                    return false;
                }
            }
            _ => return false,
        }
    }

    has_any_wider
}

fn is_object_wider_than_literal(
    annotated: &Type,
    ann_obj: &biome_js_type_info::Object,
    inf_lit: &biome_js_type_info::ObjectLiteral,
    depth: u8,
) -> bool {
    if ann_obj.members.is_empty() || inf_lit.members().is_empty() {
        return false;
    }

    let mut has_any_wider = false;

    for ann_member in ann_obj.members.iter() {
        let ann_name = match &ann_member.kind {
            biome_js_type_info::TypeMemberKind::Named(name) => name,
            _ => continue,
        };

        let inf_member = inf_lit.members().iter().find(|m| m.kind.has_name(ann_name));
        let Some(inf_member) = inf_member else {
            return false;
        };

        let ann_prop_ty = annotated.resolve(&ann_member.ty);
        let inf_prop_ty = annotated.resolve(&inf_member.ty);

        match (ann_prop_ty, inf_prop_ty) {
            (Some(a), Some(b)) => {
                if types_match(&a, &b) {
                    continue;
                }
                if is_wider_than(&a, &b, depth + 1) {
                    has_any_wider = true;
                } else {
                    return false;
                }
            }
            _ => return false,
        }
    }

    has_any_wider
}
