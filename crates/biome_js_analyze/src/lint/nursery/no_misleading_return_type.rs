use biome_analyze::{Rule, RuleDiagnostic, RuleDomain, context::RuleContext, declare_lint_rule};
use biome_console::markup;
use biome_js_syntax::{
    AnyJsExpression, AnyJsFunction, AnyJsFunctionBody, AnyJsGetter, JsFunctionBody,
    JsGetterClassMember, JsGetterObjectMember, JsMethodClassMember, JsMethodObjectMember,
    JsReturnStatement, JsSyntaxKind, JsSyntaxNode, JsVariableDeclarator, JsVariableStatement,
    TsAsExpression, TsMethodSignatureClassMember, TsTypeAssertionExpression,
};
use biome_js_type_info::{Literal, Type, TypeData};
use biome_rowan::{AstNode, TextRange, TokenText, declare_node_union};
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
    /// ```ts,expect_diagnostic,file=invalid.ts
    /// function getStatus(b: boolean): string { if (b) return "loading"; return "idle"; }
    /// ```
    ///
    /// ```ts,expect_diagnostic,file=invalid2.ts
    /// function getCode(ok: boolean): number { if (ok) return 200; return 404; }
    /// ```
    ///
    /// ```ts,expect_diagnostic,file=invalid3.ts
    /// class Foo { getStatus(b: boolean): string { if (b) return "loading"; return "idle"; } }
    /// ```
    ///
    /// ```ts,expect_diagnostic,file=invalid4.ts
    /// const obj = { getMode(b: boolean): string { if (b) return "dark"; return "light"; } };
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
    ///
    /// ```ts
    /// class Foo { greet(): string { return "hello"; } }
    /// ```
    ///
    /// ## Known limitations
    ///
    /// Suggested replacement types are only shown when their textual
    /// representation is up to 80 characters long. Longer unions fall back to
    /// a generic note without the specific suggestion.
    pub NoMisleadingReturnType {
        version: "2.4.11",
        name: "noMisleadingReturnType",
        language: "ts",
        recommended: false,
        domains: &[RuleDomain::Types],
        issue_number: Some("9810"),
    }
}

declare_node_union! {
    pub AnyFunctionLikeWithReturnType =
        AnyJsFunction
        | JsMethodClassMember
        | JsMethodObjectMember
        | JsGetterClassMember
        | JsGetterObjectMember
}

pub struct RuleState {
    annotation_range: TextRange,
    returns: Vec<Type>,
    effective_return_ty: Type,
    has_any_const_return: bool,
}

enum DescriptionKind {
    /// Built from the actual return values (e.g. `"loading" | "idle"`).
    Inferred,
    /// Built by narrowing the annotation's union to its covered variants.
    Narrowed,
}

impl RuleState {
    /// Returns the suggestion kind to render, or `None` to fall back to the
    /// generic note.
    fn description_kind(&self) -> Option<DescriptionKind> {
        if self.has_any_const_return {
            return can_render_inferred(&self.returns).then_some(DescriptionKind::Inferred);
        }
        if can_render_narrowed(&self.effective_return_ty, &self.returns) {
            return Some(DescriptionKind::Narrowed);
        }
        can_render_inferred(&self.returns).then_some(DescriptionKind::Inferred)
    }
}

impl biome_console::fmt::Display for RuleState {
    fn fmt(&self, formatter: &mut biome_console::fmt::Formatter<'_>) -> std::io::Result<()> {
        match self.description_kind() {
            Some(DescriptionKind::Inferred) => write_inferred(formatter, &self.returns),
            Some(DescriptionKind::Narrowed) => {
                write_narrowed(formatter, &self.effective_return_ty, &self.returns)
            }
            None => Ok(()),
        }
    }
}

/// Maximum iterations for type graph traversal to guard against infinite loops on cyclic types.
const MAX_TYPE_TRAVERSAL_ITERATIONS: usize = 50;

/// Upper bound on a narrowed return-type suggestion; longer unions fall back
/// to the generic diagnostic note.
const MAX_DESCRIPTION_LENGTH: usize = 80;

/// Separator rendered between union members in the suggestion note.
const SEPARATOR: &str = " | ";

impl Rule for NoMisleadingReturnType {
    type Query = Typed<AnyFunctionLikeWithReturnType>;
    type State = RuleState;
    type Signals = Option<Self::State>;
    type Options = NoMisleadingReturnTypeOptions;

    fn run(ctx: &RuleContext<Self>) -> Self::Signals {
        let node = ctx.query();

        match node {
            AnyFunctionLikeWithReturnType::AnyJsFunction(func) => {
                run_for_function(ctx, func)
            }
            AnyFunctionLikeWithReturnType::JsMethodClassMember(method) => {
                if method.star_token().is_some() {
                    return None;
                }
                if is_class_method_overload_implementation(method) {
                    return None;
                }
                let annotation = method.return_type_annotation()?;
                let name = method.name().ok()?.as_js_literal_member_name()?.name().ok()?;
                let func_type = ctx.type_of_member(method.syntax(), name.text());
                run_for_member(ctx, annotation.range(), &func_type, method.async_token().is_some(), &method.body().ok()?)
            }
            AnyFunctionLikeWithReturnType::JsMethodObjectMember(method) => {
                if method.star_token().is_some() {
                    return None;
                }
                let annotation = method.return_type_annotation()?;
                let name = method.name().ok()?.as_js_literal_member_name()?.name().ok()?;
                let func_type = ctx.type_of_member(method.syntax(), name.text());
                run_for_member(ctx, annotation.range(), &func_type, method.async_token().is_some(), &method.body().ok()?)
            }
            AnyFunctionLikeWithReturnType::JsGetterClassMember(getter) => {
                let annotation = getter.return_type()?;
                let any_getter = AnyJsGetter::from(getter.clone());
                let name = any_getter.member_name()?;
                if any_getter.has_matching_setter(&name) {
                    return None;
                }
                let func_type = ctx.type_of_member(getter.syntax(), name.text());
                run_for_member(ctx, annotation.range(), &func_type, false, &getter.body().ok()?)
            }
            AnyFunctionLikeWithReturnType::JsGetterObjectMember(getter) => {
                let annotation = getter.return_type()?;
                let any_getter = AnyJsGetter::from(getter.clone());
                let name = any_getter.member_name()?;
                if any_getter.has_matching_setter(&name) {
                    return None;
                }
                let func_type = ctx.type_of_member(getter.syntax(), name.text());
                run_for_member(ctx, annotation.range(), &func_type, false, &getter.body().ok()?)
            }
        }
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

        let diag = if state.description_kind().is_some() {
            diag.note(markup! {
                "Consider using "{state}" as the return type."
            })
        } else {
            diag.note(markup! {
                "Narrow the return type to match what the function actually returns."
            })
        };

        Some(diag)
    }

}

/// Looks for sibling function declarations with the same name but no body,
/// which indicates this function is the implementation of an overload set.
/// Overload signatures are parsed as `TsDeclareFunctionDeclaration` or as
/// `AnyJsFunction` with `body().is_err()`.
fn is_overload_implementation(node: &AnyJsFunction) -> bool {
    let name = node
        .binding()
        .and_then(|b| b.as_js_identifier_binding().cloned())
        .and_then(|id| id.name_token().ok())
        .map(|t| t.token_text_trimmed());
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
                .is_some_and(|t| t.token_text_trimmed() == name);
        }
        AnyJsFunction::cast(sibling).is_some_and(|sib_fn| {
            sib_fn.body().is_err()
                && sib_fn
                    .binding()
                    .and_then(|b| b.as_js_identifier_binding().cloned())
                    .and_then(|id| id.name_token().ok())
                    .is_some_and(|t| t.token_text_trimmed() == name)
        })
    })
}

fn run_for_function(
    ctx: &RuleContext<NoMisleadingReturnType>,
    node: &AnyJsFunction,
) -> Option<RuleState> {
    let annotation = node.return_type_annotation()?;
    let annotation_range = annotation.range();

    if node.is_generator() || is_overload_implementation(node) {
        return None;
    }

    let func_type = ctx.type_of_function(node);
    let is_async = node.async_token().is_some();
    let body = node.body().ok()?;

    run_for_member_with_body(ctx, annotation_range, &func_type, is_async, &body)
}

fn run_for_member(
    ctx: &RuleContext<NoMisleadingReturnType>,
    annotation_range: TextRange,
    func_type: &Type,
    is_async: bool,
    body: &JsFunctionBody,
) -> Option<RuleState> {
    run_for_member_with_body(
        ctx,
        annotation_range,
        func_type,
        is_async,
        &AnyJsFunctionBody::JsFunctionBody(body.clone()),
    )
}

fn run_for_member_with_body(
    ctx: &RuleContext<NoMisleadingReturnType>,
    annotation_range: TextRange,
    func_type: &Type,
    is_async: bool,
    body: &AnyJsFunctionBody,
) -> Option<RuleState> {
    let return_ty = extract_return_type(func_type)?;

    if is_escape_hatch(&return_ty) {
        return None;
    }

    let effective_return_ty = if is_async {
        unwrap_promise_inner(&return_ty)
    } else {
        return_ty.clone()
    };

    // Normalize `"a" | "b" | string` → `string` before widening checks.
    let effective_return_ty =
        collapse_union_absorbed_by_primitive(&effective_return_ty).unwrap_or(effective_return_ty);

    let (returns, has_any_const_return) = collect_return_info(ctx, body);

    if returns.is_empty() {
        return None;
    }

    if is_single_literal_primitive_return(&returns)
        && !has_any_const_return
        && !effective_return_ty.is_union()
    {
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

    // tsc collapses any union containing `unknown` to `unknown`.
    if matches!(&*effective_return_ty, TypeData::Union(_))
        && effective_return_ty
            .flattened_union_variants()
            .any(|v| matches!(&*v, TypeData::UnknownKeyword | TypeData::Unknown))
    {
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
        && is_only_property_literal_widening(&effective_return_ty, &returns)
    {
        return None;
    }

    let is_misleading = if effective_return_ty.is_union() {
        is_union_wider_than_returns(&effective_return_ty, &returns)
    } else {
        returns
            .iter()
            .all(|inferred| is_wider_than(&effective_return_ty, inferred))
    };

    if !is_misleading {
        return None;
    }

    Some(RuleState {
        annotation_range,
        returns,
        effective_return_ty,
        has_any_const_return,
    })
}

fn is_class_method_overload_implementation(method: &JsMethodClassMember) -> bool {
    let name = method
        .name()
        .ok()
        .and_then(|n| n.as_js_literal_member_name().cloned())
        .and_then(|n| n.value().ok())
        .map(|t| t.token_text_trimmed());
    let Some(name) = name else { return false };

    let Some(member_list) = method.syntax().parent() else {
        return false;
    };

    member_list.children().any(|child| {
        child.kind() == JsSyntaxKind::TS_METHOD_SIGNATURE_CLASS_MEMBER
            && TsMethodSignatureClassMember::cast(child)
                .and_then(|sig| sig.name().ok())
                .and_then(|n| n.as_js_literal_member_name().cloned())
                .and_then(|n| n.value().ok())
                .map(|t| t.token_text_trimmed())
                .is_some_and(|sig_name| sig_name == name)
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

/// Returns the primitive a union collapses to at the TS level, when exactly
/// one variant is a primitive and every other variant is a literal of it.
fn collapse_union_absorbed_by_primitive(ty: &Type) -> Option<Type> {
    if !matches!(&**ty, TypeData::Union(_)) {
        return None;
    }
    let variants: Vec<Type> = ty.flattened_union_variants().collect();
    let mut primitive: Option<Type> = None;
    for variant in &variants {
        if matches!(
            &**variant,
            TypeData::String | TypeData::Number | TypeData::Boolean | TypeData::BigInt
        ) {
            if primitive.is_some() {
                return None;
            }
            primitive = Some(variant.clone());
        }
    }
    let primitive = primitive?;
    let all_absorbed = variants.iter().all(|variant| {
        types_match(variant, &primitive) || is_nonunion_wider(&primitive, variant)
    });
    all_absorbed.then_some(primitive)
}

/// For async functions the annotation is `Promise<T>`. We need `T` to compare
/// against the return expressions, which are not wrapped in `Promise`.
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
    match &**ty {
        TypeData::Literal(lit) => lit.is_primitive(),
        // The type resolver may wrap a single literal in a Union for mutable
        // bindings.  Treat a one-element union of a primitive literal the same.
        TypeData::Union(_) => {
            let mut iter = ty.flattened_union_variants();
            matches!(
                (iter.next(), iter.next()),
                (Some(v), None) if matches!(&*v, TypeData::Literal(lit) if lit.is_primitive())
            )
        }
        _ => false,
    }
}

fn is_single_literal_primitive_return(returns: &[Type]) -> bool {
    returns.len() == 1 && is_literal_of_primitive(&returns[0])
}

/// Checks whether annotation differs from returns only by property-level
/// literal widening that contextual typing would handle.
fn is_only_property_literal_widening(annotation: &Type, returns: &[Type]) -> bool {
    returns.iter().all(|inferred| {
        let mut stack: Vec<(Type, Type)> = vec![(annotation.clone(), inferred.clone())];
        let mut has_widening = false;
        let mut iterations = 0usize;

        while let Some((annotated, inferred)) = stack.pop() {
            iterations += 1;
            if iterations > MAX_TYPE_TRAVERSAL_ITERATIONS {
                return false;
            }

            if let TypeData::Tuple(annotated_tuple) = &*annotated {
                let TypeData::Tuple(inferred_tuple) = &*inferred else {
                    return false;
                };
                let annotated_elements = annotated_tuple.elements();
                let inferred_elements = inferred_tuple.elements();
                if annotated_elements.len() != inferred_elements.len()
                    || annotated_elements.is_empty()
                {
                    return false;
                }
                for (annotated_element, inferred_element) in
                    annotated_elements.iter().zip(inferred_elements.iter())
                {
                    match (
                        annotated.resolve(&annotated_element.ty),
                        inferred.resolve(&inferred_element.ty),
                    ) {
                        (Some(annotated_type), Some(inferred_type)) => {
                            if types_match(&annotated_type, &inferred_type) {
                                continue;
                            }
                            if is_base_type_of_literal(&annotated_type, &inferred_type) {
                                has_widening = true;
                            } else {
                                stack.push((annotated_type, inferred_type));
                            }
                        }
                        _ => return false,
                    }
                }
                continue;
            }

            let TypeData::Object(annotated_object) = &*annotated else {
                return false;
            };
            if annotated_object.members.is_empty() {
                return false;
            }

            let inferred_members = match &*inferred {
                TypeData::Object(object) => &object.members,
                TypeData::Literal(literal) => match literal.as_ref() {
                    Literal::Object(object_literal) => object_literal.members(),
                    _ => return false,
                },
                _ => return false,
            };
            if inferred_members.is_empty() {
                return false;
            }

            let annotated_index_signature = annotated_object.members.iter().find(|member| {
                matches!(
                    member.kind,
                    biome_js_type_info::TypeMemberKind::IndexSignature(_)
                )
            });
            if let Some(index_signature_member) = annotated_index_signature
                && let Some(index_signature_value_type) =
                    annotated.resolve(&index_signature_member.ty)
            {
                let mut index_signature_has_widening = false;
                let all_inferred_covered = inferred_members.iter().all(|inferred_member| {
                    if let Some(inferred_type) = inferred.resolve(&inferred_member.ty) {
                        if types_match(&index_signature_value_type, &inferred_type) {
                            return true;
                        }
                        if is_base_type_of_literal(&index_signature_value_type, &inferred_type) {
                            index_signature_has_widening = true;
                            return true;
                        }
                    }
                    false
                });
                if !(all_inferred_covered && index_signature_has_widening) {
                    return false;
                }
                has_widening = true;
                continue;
            }

            for annotated_member in annotated_object.members.iter() {
                let annotated_name = match &annotated_member.kind {
                    biome_js_type_info::TypeMemberKind::Named(name)
                    | biome_js_type_info::TypeMemberKind::NamedOptional(name) => name,
                    _ => continue,
                };
                let Some(inferred_member) = inferred_members
                    .iter()
                    .find(|member| member.kind.has_name(annotated_name))
                else {
                    return false;
                };
                match (
                    annotated.resolve(&annotated_member.ty),
                    inferred.resolve(&inferred_member.ty),
                ) {
                    (Some(annotated_type), Some(inferred_type)) => {
                        if types_match(&annotated_type, &inferred_type) {
                            continue;
                        }
                        if is_base_type_of_literal(&annotated_type, &inferred_type) {
                            has_widening = true;
                        } else {
                            stack.push((annotated_type, inferred_type));
                        }
                    }
                    _ => return false,
                }
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

/// Rendered byte length of a string/number/boolean literal; `None` otherwise.
fn literal_display_len(literal: &Literal) -> Option<usize> {
    match literal {
        Literal::String(value) => Some(value.as_str().len() + "\"\"".len()),
        Literal::Number(value) => Some(value.as_str().len()),
        Literal::Boolean(value) => {
            Some(if value.as_bool() { "true".len() } else { "false".len() })
        }
        _ => None,
    }
}

/// Rejects literals whose rendered text contains `...`, `__internal`, or `typeof import(`.
fn literal_content_ok(literal: &Literal) -> bool {
    let text = match literal {
        Literal::String(value) => value.as_str(),
        Literal::Number(value) => value.as_str(),
        _ => return true,
    };
    !text.contains("...") && !text.contains("__internal") && !text.contains("typeof import(")
}

/// Writes a string/number/boolean literal; writes nothing for other kinds.
fn write_literal(
    formatter: &mut biome_console::fmt::Formatter<'_>,
    literal: &Literal,
) -> std::io::Result<()> {
    match literal {
        Literal::String(value) => write!(formatter, "\"{}\"", value.as_str()),
        Literal::Number(value) => formatter.write_str(value.as_str()),
        Literal::Boolean(value) => formatter.write_str(if value.as_bool() { "true" } else { "false" }),
        _ => Ok(()),
    }
}

/// Rendered byte length of a primitive keyword or literal variant; `None` otherwise.
fn variant_display_len(variant: &Type) -> Option<usize> {
    match &**variant {
        TypeData::String => Some("string".len()),
        TypeData::Number => Some("number".len()),
        TypeData::BigInt => Some("bigint".len()),
        TypeData::Boolean => Some("boolean".len()),
        TypeData::Literal(literal) => literal_display_len(literal.as_ref()),
        _ => None,
    }
}

/// Writes a primitive keyword or literal variant; writes nothing for other kinds.
fn write_variant(
    formatter: &mut biome_console::fmt::Formatter<'_>,
    variant: &Type,
) -> std::io::Result<()> {
    match &**variant {
        TypeData::String => formatter.write_str("string"),
        TypeData::Number => formatter.write_str("number"),
        TypeData::Boolean => formatter.write_str("boolean"),
        TypeData::BigInt => formatter.write_str("bigint"),
        TypeData::Literal(literal) => write_literal(formatter, literal.as_ref()),
        _ => Ok(()),
    }
}

/// `true` when every return is a displayable literal with clean content and
/// the joined output fits within [`MAX_DESCRIPTION_LENGTH`].
fn can_render_inferred(returns: &[Type]) -> bool {
    let mut total_len = 0usize;
    let mut has_any = false;
    for return_type in returns {
        let TypeData::Literal(literal) = &**return_type else {
            return false;
        };
        if !literal_content_ok(literal.as_ref()) {
            return false;
        }
        let Some(literal_len) = literal_display_len(literal.as_ref()) else {
            return false;
        };
        if has_any {
            total_len += SEPARATOR.len();
        }
        total_len += literal_len;
        has_any = true;
    }
    has_any && total_len <= MAX_DESCRIPTION_LENGTH
}

/// Writes an inferred description like `"loading" | "idle"`.
fn write_inferred(
    formatter: &mut biome_console::fmt::Formatter<'_>,
    returns: &[Type],
) -> std::io::Result<()> {
    let mut first = true;
    for return_type in returns {
        if let TypeData::Literal(literal) = &**return_type {
            if !first {
                formatter.write_str(SEPARATOR)?;
            }
            write_literal(formatter, literal.as_ref())?;
            first = false;
        }
    }
    Ok(())
}

/// Returns `true` when the annotation's union can be safely narrowed to only
/// its covered variants within [`MAX_DESCRIPTION_LENGTH`].
fn can_render_narrowed(annotation: &Type, returns: &[Type]) -> bool {
    let covers_any = |variant: &Type| {
        returns.iter().any(|return_type| {
            types_match(variant, return_type) || is_nonunion_wider(variant, return_type)
        })
    };

    let mut total = 0usize;
    let mut covered = 0usize;
    let mut all_renderable = true;
    let mut has_widening = false;
    let mut render_len = 0usize;
    let mut first = true;

    for variant in annotation.flattened_union_variants() {
        total += 1;
        if !covers_any(&variant) {
            continue;
        }
        covered += 1;
        if returns.iter().any(|return_type| {
            !types_match(&variant, return_type) && is_nonunion_wider(&variant, return_type)
        }) {
            has_widening = true;
        }
        match variant_display_len(&variant) {
            Some(len) => {
                if !first {
                    render_len += SEPARATOR.len();
                }
                render_len += len;
                first = false;
            }
            None => all_renderable = false,
        }
    }

    if covered == 0 || covered == total || !all_renderable {
        return false;
    }

    // A widening variant would keep the narrowed annotation misleading, unless
    // the single-literal-primitive bailout upstream would hide it.
    let single_literal_bailout =
        covered == 1 && is_single_literal_primitive_return(returns);
    if has_widening && !single_literal_bailout {
        return false;
    }

    render_len <= MAX_DESCRIPTION_LENGTH
}

/// Writes narrowed annotation variants (only those covered by the returns).
fn write_narrowed(
    formatter: &mut biome_console::fmt::Formatter<'_>,
    annotation: &Type,
    returns: &[Type],
) -> std::io::Result<()> {
    let covers_any = |variant: &Type| {
        returns.iter().any(|return_type| {
            types_match(variant, return_type) || is_nonunion_wider(variant, return_type)
        })
    };
    let mut first = true;
    for variant in annotation.flattened_union_variants().filter(covers_any) {
        if !first {
            formatter.write_str(SEPARATOR)?;
        }
        write_variant(formatter, &variant)?;
        first = false;
    }
    Ok(())
}

/// Collects return types and tracks `as const` usage from a function body.
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

    for node in block
        .syntax()
        .pruned_descendents(|n| !is_nested_function_like(n))
    {
        let Some(ret) = JsReturnStatement::cast(node) else {
            continue;
        };
        if let Some(arg) = ret.argument()
            && let Some(expr) = AnyJsExpression::cast(arg.syntax().clone())
        {
            if has_const_assertion(&expr) {
                *has_any_const = true;
            }
            returns.push(infer_expression_type(ctx, &expr));
        }
    }

    returns
}

/// Gets the type of a return expression. For identifiers bound to an
/// `as const` initializer, walks the AST to find the original literal type
/// since `type_of_expression` would return the widened type.
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
        .map(|t| t.token_text_trimmed())?;

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
                .map(|t| t.token_text_trimmed());
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
    let mut current = expr.clone();
    loop {
        match &current {
            AnyJsExpression::TsAsExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return current,
            },
            AnyJsExpression::TsSatisfiesExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return current,
            },
            AnyJsExpression::TsTypeAssertionExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return current,
            },
            AnyJsExpression::JsParenthesizedExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return current,
            },
            _ => return current,
        }
    }
}

fn has_const_assertion(expr: &AnyJsExpression) -> bool {
    let mut current = expr.clone();
    loop {
        match &current {
            AnyJsExpression::TsAsExpression(e) => return is_const_type_assertion(e),
            AnyJsExpression::TsTypeAssertionExpression(e) => {
                return is_const_angle_bracket_assertion(e)
            }
            AnyJsExpression::JsParenthesizedExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return false,
            },
            AnyJsExpression::TsSatisfiesExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return false,
            },
            AnyJsExpression::JsIdentifierExpression(id_expr) => {
                return identifier_refers_to_const_assertion(id_expr)
            }
            _ => return false,
        }
    }
}

fn identifier_refers_to_const_assertion(
    id_expr: &biome_js_syntax::JsIdentifierExpression,
) -> bool {
    let name = id_expr
        .name()
        .ok()
        .and_then(|n| n.value_token().ok())
        .map(|t| t.token_text_trimmed());
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

fn declarator_matches_name_with_const(declarator: &JsVariableDeclarator, name: &TokenText) -> bool {
    let id_text = declarator
        .id()
        .ok()
        .and_then(|id| id.as_any_js_binding().cloned())
        .and_then(|b| b.as_js_identifier_binding().cloned())
        .and_then(|ib| ib.name_token().ok())
        .map(|t| t.token_text_trimmed());
    let Some(id_text) = id_text else { return false };

    if id_text != *name {
        return false;
    }

    // We already resolved the identifier to reach this declarator,
    // so there's no need to follow identifiers again.
    declarator
        .initializer()
        .and_then(|init| init.expression().ok())
        .is_some_and(|init_expr| init_has_direct_const_assertion(&init_expr))
}

/// Checks for `as const` on the expression itself, without following identifiers.
fn init_has_direct_const_assertion(expr: &AnyJsExpression) -> bool {
    let mut current = expr.clone();
    loop {
        match &current {
            AnyJsExpression::TsAsExpression(e) => return is_const_type_assertion(e),
            AnyJsExpression::TsTypeAssertionExpression(e) => {
                return is_const_angle_bracket_assertion(e)
            }
            AnyJsExpression::JsParenthesizedExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return false,
            },
            AnyJsExpression::TsSatisfiesExpression(e) => match e.expression() {
                Ok(inner) => current = inner,
                Err(_) => return false,
            },
            _ => return false,
        }
    }
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

/// Follows generic constraints iteratively: `T extends U extends string` → `string`.
fn resolve_generic_chain(ty: &Type) -> Type {
    let mut current = ty.clone();
    let mut steps = 0u8;
    while let TypeData::Generic(generic) = &*current {
        if steps > 5 || !generic.constraint.is_known() {
            break;
        }
        match current.resolve(&generic.constraint) {
            Some(resolved) => {
                current = resolved;
                steps += 1;
            }
            None => break,
        }
    }
    current
}

/// Compares non-union type pairs using a work stack. Compound types
/// (Instance params, Object properties) are decomposed into sub-pairs
/// and pushed back onto the stack for further comparison.
fn is_nonunion_wider(annotated: &Type, inferred: &Type) -> bool {
    let mut stack: Vec<(Type, Type)> =
        vec![(annotated.clone(), resolve_generic_chain(inferred))];
    let mut found_wider = false;
    let mut iterations = 0usize;

    while let Some((ann, inf)) = stack.pop() {
        iterations += 1;
        if iterations > MAX_TYPE_TRAVERSAL_ITERATIONS {
            return false;
        }

        if is_base_type_of_literal(&ann, &inf) {
            found_wider = true;
            continue;
        }

        if types_match(&ann, &inf) {
            continue;
        }

        match (&*ann, &*inf) {
            (TypeData::InstanceOf(ann_inst), TypeData::InstanceOf(inf_inst)) => {
                let same_base = match (ann.resolve(&ann_inst.ty), inf.resolve(&inf_inst.ty)) {
                    (Some(a), Some(b)) => types_match(&a, &b),
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
                for (ann_p, inf_p) in ann_params.iter().zip(inf_params.iter()) {
                    match (ann.resolve(ann_p), inf.resolve(inf_p)) {
                        (Some(a), Some(b)) => stack.push((a, resolve_generic_chain(&b))),
                        _ => return false,
                    }
                }
            }

            (TypeData::Object(ann_obj), TypeData::Object(inf_obj)) => {
                if !push_object_pairs(&ann, ann_obj, &inf, inf_obj, &mut stack) {
                    return false;
                }
            }

            (TypeData::Object(ann_obj), TypeData::Literal(lit)) => match lit.as_ref() {
                Literal::Object(inf_lit) => {
                    if !push_object_literal_pairs(&ann, ann_obj, inf_lit, &mut stack) {
                        return false;
                    }
                }
                _ => return false,
            },

            (TypeData::Tuple(ann_tuple), TypeData::Tuple(inf_tuple)) => {
                let ann_elems = ann_tuple.elements();
                let inf_elems = inf_tuple.elements();
                if ann_elems.len() != inf_elems.len() || ann_elems.is_empty() {
                    return false;
                }
                for (ann_e, inf_e) in ann_elems.iter().zip(inf_elems.iter()) {
                    match (ann.resolve(&ann_e.ty), inf.resolve(&inf_e.ty)) {
                        (Some(a), Some(b)) => stack.push((a, resolve_generic_chain(&b))),
                        _ => return false,
                    }
                }
            }

            _ => return false,
        }
    }

    found_wider
}

/// Pushes property type pairs onto the work stack for pairwise comparison.
/// Also handles index signatures, which arise from `Record<K,V>` annotations.
fn push_object_pairs(
    annotated: &Type,
    ann_obj: &biome_js_type_info::Object,
    inferred: &Type,
    inf_obj: &biome_js_type_info::Object,
    stack: &mut Vec<(Type, Type)>,
) -> bool {
    if ann_obj.members.is_empty() || inf_obj.members.is_empty() {
        return false;
    }

    let ann_index_sig = ann_obj.members.iter().find(|m| {
        matches!(m.kind, biome_js_type_info::TypeMemberKind::IndexSignature(_))
    });
    if let Some(sig_member) = ann_index_sig
        && let Some(sig_value_ty) = annotated.resolve(&sig_member.ty)
    {
        for inf_m in inf_obj.members.iter() {
            match inferred.resolve(&inf_m.ty) {
                Some(inf_ty) => stack.push((sig_value_ty.clone(), resolve_generic_chain(&inf_ty))),
                None => return false,
            }
        }
        return true;
    }

    for ann_member in ann_obj.members.iter() {
        let ann_name = match &ann_member.kind {
            biome_js_type_info::TypeMemberKind::Named(name)
            | biome_js_type_info::TypeMemberKind::NamedOptional(name) => name,
            _ => continue,
        };
        let inf_member = inf_obj.members.iter().find(|m| m.kind.has_name(ann_name));
        let Some(inf_member) = inf_member else {
            return false;
        };
        match (annotated.resolve(&ann_member.ty), inferred.resolve(&inf_member.ty)) {
            (Some(a), Some(b)) => stack.push((a, resolve_generic_chain(&b))),
            _ => return false,
        }
    }

    true
}

fn push_object_literal_pairs(
    annotated: &Type,
    ann_obj: &biome_js_type_info::Object,
    inf_lit: &biome_js_type_info::ObjectLiteral,
    stack: &mut Vec<(Type, Type)>,
) -> bool {
    if ann_obj.members.is_empty() || inf_lit.members().is_empty() {
        return false;
    }

    for ann_member in ann_obj.members.iter() {
        let ann_name = match &ann_member.kind {
            biome_js_type_info::TypeMemberKind::Named(name)
            | biome_js_type_info::TypeMemberKind::NamedOptional(name) => name,
            _ => continue,
        };
        let inf_member = inf_lit.members().iter().find(|m| m.kind.has_name(ann_name));
        let Some(inf_member) = inf_member else {
            return false;
        };
        match (annotated.resolve(&ann_member.ty), annotated.resolve(&inf_member.ty)) {
            (Some(a), Some(b)) => stack.push((a, resolve_generic_chain(&b))),
            _ => return false,
        }
    }

    true
}

/// Checks whether `annotated` is strictly wider than `inferred`.
fn is_wider_than(annotated: &Type, inferred: &Type) -> bool {
    let current = resolve_generic_chain(inferred);

    match (&**annotated, &*current) {
        (TypeData::String, TypeData::String)
        | (TypeData::Number, TypeData::Number)
        | (TypeData::Boolean, TypeData::Boolean)
        | (TypeData::BigInt, TypeData::BigInt) => false,

        (TypeData::Union(_), _) => is_union_wider(annotated, &current),
        (_, TypeData::Union(_)) => {
            // When the annotation's base type already appears as a variant in the
            // inferred union, any literal subtypes are subsumed by it — the union
            // collapses to the base type (e.g., 0 | number = number).  In that
            // case the annotation is not wider than the inferred type.
            let (has_base_variant, all_subsumed, all_covered, any_wider) = current
                .flattened_union_variants()
                .fold(
                    (false, true, true, false),
                    |(has_base_variant, all_subsumed, all_covered, any_wider), v| {
                        let matches = types_match(annotated, &v);
                        let wider = is_nonunion_wider(annotated, &v);
                        (
                            has_base_variant || matches,
                            all_subsumed && (matches || is_base_type_of_literal(annotated, &v)),
                            all_covered && (matches || wider),
                            any_wider || wider,
                        )
                    },
                );
            if has_base_variant && all_subsumed {
                return false;
            }
            all_covered && any_wider
        }
        _ => is_nonunion_wider(annotated, &current),
    }
}

/// Flags when the annotation has an unreached variant or a variant strictly
/// wider than a return that no other variant matches directly.
fn is_union_wider_than_returns(annotated: &Type, returns: &[Type]) -> bool {
    let all_covered = returns.iter().all(|ret| {
        annotated
            .flattened_union_variants()
            .any(|ann_v| types_match(&ann_v, ret) || is_nonunion_wider(&ann_v, ret))
    });

    if !all_covered {
        return false;
    }

    let variants: Vec<Type> = annotated.flattened_union_variants().collect();

    let has_extra = variants.iter().any(|ann_v| {
        !returns
            .iter()
            .any(|ret| types_match(ann_v, ret) || is_nonunion_wider(ann_v, ret))
    });

    // A return already matched directly by another variant is not treated as
    // misleadingly widened.
    let has_wider_variant = returns.iter().any(|ret| {
        !variants.iter().any(|ann_v| types_match(ann_v, ret))
            && variants.iter().any(|ann_v| is_nonunion_wider(ann_v, ret))
    });

    has_extra || has_wider_variant
}

/// Like `is_union_wider_than_returns` but for a single inferred type (used
/// inside `is_wider_than`). Also filters out generic variants whose
/// constraints are subsumed by other variants in the annotation union.
fn is_union_wider(annotated: &Type, inferred: &Type) -> bool {
    let all_inferred_covered = if let TypeData::Union(_) = &**inferred {
        inferred.flattened_union_variants().all(|inf_v| {
            annotated
                .flattened_union_variants()
                .any(|ann_v| types_match(&ann_v, &inf_v) || is_nonunion_wider(&ann_v, &inf_v))
        })
    } else {
        annotated
            .flattened_union_variants()
            .any(|ann_v| types_match(&ann_v, inferred) || is_nonunion_wider(&ann_v, inferred))
    };

    if !all_inferred_covered {
        return false;
    }

    let ann_variants: Vec<Type> = annotated.flattened_union_variants().collect();

    let inf_variants: Vec<Type> = match &**inferred {
        TypeData::Union(_) => inferred.flattened_union_variants().collect(),
        _ => vec![inferred.clone()],
    };

    ann_variants
        .iter()
        .filter(|ann_v| {
            if let TypeData::Generic(generic) = &***ann_v
                && generic.constraint.is_known()
                && let Some(constraint) = ann_v.resolve(&generic.constraint)
            {
                let subsumed = ann_variants.iter().any(|other| {
                    !std::ptr::eq(*ann_v as *const Type, other as *const Type)
                        && (types_match(other, &constraint)
                            || is_nonunion_wider(other, &constraint))
                });
                return !subsumed;
            }
            true
        })
        .any(|ann_v| {
            !inf_variants
                .iter()
                .any(|inf_v| types_match(ann_v, inf_v) || is_nonunion_wider(ann_v, inf_v))
        })
}

/// Checks structural equality between two types.
fn types_match(a: &Type, b: &Type) -> bool {
    let mut a = a.clone();
    let mut b = b.clone();
    loop {
        match (&*a, &*b) {
            (TypeData::String, TypeData::String)
            | (TypeData::Number, TypeData::Number)
            | (TypeData::Boolean, TypeData::Boolean)
            | (TypeData::BigInt, TypeData::BigInt)
            | (TypeData::Null, TypeData::Null)
            | (TypeData::Undefined, TypeData::Undefined)
            | (TypeData::VoidKeyword, TypeData::VoidKeyword)
            | (TypeData::NeverKeyword, TypeData::NeverKeyword) => return true,

            (TypeData::Literal(a_lit), TypeData::Literal(b_lit)) => return a_lit == b_lit,

            (TypeData::Generic(a_gen), TypeData::Generic(b_gen)) => {
                return a_gen.name == b_gen.name
            }

            (TypeData::InstanceOf(a_inst), TypeData::InstanceOf(b_inst))
                if a_inst.type_parameters.is_empty() && b_inst.type_parameters.is_empty() =>
            {
                match (a.resolve(&a_inst.ty), b.resolve(&b_inst.ty)) {
                    (Some(a_base), Some(b_base)) => {
                        a = a_base;
                        b = b_base;
                    }
                    _ => return false,
                }
            }

            (TypeData::Generic(a_gen), TypeData::InstanceOf(b_inst))
                if b_inst.type_parameters.is_empty() =>
            {
                if let Some(base) = b.resolve(&b_inst.ty)
                    && let TypeData::Generic(b_gen) = &*base
                {
                    return a_gen.name == b_gen.name;
                }
                return false;
            }
            (TypeData::InstanceOf(a_inst), TypeData::Generic(b_gen))
                if a_inst.type_parameters.is_empty() =>
            {
                if let Some(base) = a.resolve(&a_inst.ty)
                    && let TypeData::Generic(a_gen) = &*base
                {
                    return a_gen.name == b_gen.name;
                }
                return false;
            }

            _ => return false,
        }
    }
}
