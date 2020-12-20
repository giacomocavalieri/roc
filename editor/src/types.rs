#![allow(clippy::all)]
#![allow(dead_code)]
#![allow(unused_imports)]
use crate::expr::Env;
use crate::pool::{NodeId, Pool, PoolStr, PoolVec};
use crate::scope::Scope;
// use roc_can::expr::Output;
use roc_collections::all::{MutMap, MutSet};
use roc_module::ident::{Ident, TagName};
use roc_module::symbol::Symbol;
use roc_region::all::{Located, Region};
use roc_types::subs::Variable;
use roc_types::types::{Problem, RecordField};

pub type TypeId = NodeId<Type2>;

#[derive(Debug)]
pub enum Type2 {
    Variable(Variable),

    Alias(Symbol, PoolVec<(PoolStr, TypeId)>, TypeId), // 20B = 8B + 8B + 4B
    AsAlias(Symbol, PoolVec<(PoolStr, TypeId)>, TypeId), // 20B = 8B + 8B + 4B

    HostExposedAlias {
        name: Symbol,                          // 8B
        arguments: PoolVec<(PoolStr, TypeId)>, // 8B
        actual_var: Variable,                  // 4B
        actual: TypeId,                        // 4B
    },

    EmptyTagUnion,
    TagUnion(PoolVec<(PoolStr, PoolVec<Type2>)>, TypeId),
    RecursiveTagUnion(Variable, PoolVec<(PoolStr, PoolVec<Type2>)>, TypeId),

    EmptyRec,
    Record(PoolVec<(PoolStr, RecordField<Type2>)>, TypeId),

    Function(PoolVec<Type2>, TypeId, TypeId), // 16B = 8B + 4B + 4B
    Apply(Symbol, PoolVec<Type2>),            // 16B = 8B + 8B

    Erroneous(roc_types::types::Problem),
}

impl Type2 {
    fn substitute(_pool: &mut Pool, _subs: &MutMap<Variable, TypeId>, _type_id: TypeId) {
        todo!()
    }
    pub fn variables(&self, _pool: &mut Pool) -> MutSet<Variable> {
        todo!()
    }
}

impl NodeId<Type2> {
    pub fn variables(&self, _pool: &mut Pool) -> MutSet<Variable> {
        todo!()
    }
}

/// A temporary data structure to return a bunch of values to Def construction
pub enum Signature {
    FunctionWithAliases {
        annotation: Type2,
        arguments: PoolVec<Type2>,
        closure_type_id: TypeId,
        return_type_id: TypeId,
    },
    Function {
        arguments: PoolVec<Type2>,
        closure_type_id: TypeId,
        return_type_id: TypeId,
    },
    Value {
        annotation: Type2,
    },
}

pub enum Annotation2<'a> {
    Annotation {
        named_rigids: MutMap<&'a str, Variable>,
        unnamed_rigids: MutSet<Variable>,
        symbols: MutSet<Symbol>,
        signature: Signature,
    },
    Erroneous(roc_types::types::Problem),
}

pub fn to_annotation2<'a>(
    env: &mut Env,
    scope: &mut Scope,
    annotation: &'a roc_parse::ast::TypeAnnotation<'a>,
    region: Region,
) -> Annotation2<'a> {
    let mut references = References::default();

    let annotation = to_type2(env, scope, &mut references, annotation, region);

    // we dealias until we hit a non-alias, then we either hit a function type (and produce a
    // function annotation) or anything else (and produce a value annotation)
    match annotation {
        Type2::Function(arguments, closure_type_id, return_type_id) => {
            let References {
                named,
                unnamed,
                symbols,
                ..
            } = references;

            let signature = Signature::Function {
                arguments,
                closure_type_id,
                return_type_id,
            };

            Annotation2::Annotation {
                named_rigids: named,
                unnamed_rigids: unnamed,
                symbols,
                signature,
            }
        }
        Type2::Alias(_, _, _) => {
            // most of the time, the annotation is not an alias, so this case is comparatively
            // less efficient
            shallow_dealias(env, references, annotation)
        }
        _ => {
            let References {
                named,
                unnamed,
                symbols,
                ..
            } = references;

            let signature = Signature::Value { annotation };

            Annotation2::Annotation {
                named_rigids: named,
                unnamed_rigids: unnamed,
                symbols,
                signature,
            }
        }
    }
}

fn shallow_dealias<'a>(
    env: &mut Env,
    references: References<'a>,
    annotation: Type2,
) -> Annotation2<'a> {
    let References {
        named,
        unnamed,
        symbols,
        ..
    } = references;
    let mut inner = &annotation;

    loop {
        match inner {
            Type2::Alias(_, _, actual) => {
                inner = env.pool.get(*actual);
            }
            Type2::Function(arguments, closure_type_id, return_type_id) => {
                let signature = Signature::FunctionWithAliases {
                    arguments: arguments.duplicate(),
                    closure_type_id: *closure_type_id,
                    return_type_id: *return_type_id,
                    annotation,
                };

                return Annotation2::Annotation {
                    named_rigids: named,
                    unnamed_rigids: unnamed,
                    symbols,
                    signature,
                };
            }
            _ => {
                let signature = Signature::Value { annotation };

                return Annotation2::Annotation {
                    named_rigids: named,
                    unnamed_rigids: unnamed,
                    symbols,
                    signature,
                };
            }
        }
    }
}

#[derive(Default)]
pub struct References<'a> {
    named: MutMap<&'a str, Variable>,
    unnamed: MutSet<Variable>,
    hidden: MutSet<Variable>,
    symbols: MutSet<Symbol>,
}

pub fn to_type_id<'a>(
    env: &mut Env,
    scope: &mut Scope,
    rigids: &mut References<'a>,
    annotation: &roc_parse::ast::TypeAnnotation<'a>,
    region: Region,
) -> TypeId {
    let type2 = to_type2(env, scope, rigids, annotation, region);

    env.add(type2, region)
}

pub fn as_type_id<'a>(
    env: &mut Env,
    scope: &mut Scope,
    rigids: &mut References<'a>,
    type_id: TypeId,
    annotation: &roc_parse::ast::TypeAnnotation<'a>,
    region: Region,
) {
    let type2 = to_type2(env, scope, rigids, annotation, region);

    env.pool[type_id] = type2;
    env.set_region(type_id, region);
}

pub fn to_type2<'a>(
    env: &mut Env,
    scope: &mut Scope,
    references: &mut References<'a>,
    annotation: &roc_parse::ast::TypeAnnotation<'a>,
    region: Region,
) -> Type2 {
    use roc_parse::ast::TypeAnnotation::*;

    match annotation {
        Apply(module_name, ident, targs) => {
            match to_type_apply(env, scope, references, module_name, ident, targs, region) {
                TypeApply::Apply(symbol, args) => {
                    references.symbols.insert(symbol);
                    Type2::Apply(symbol, args)
                }
                TypeApply::Alias(symbol, args, actual) => {
                    references.symbols.insert(symbol);
                    Type2::Alias(symbol, args, actual)
                }
                TypeApply::Erroneous(problem) => Type2::Erroneous(problem),
            }
        }
        Function(argument_types, return_type) => {
            let arguments = PoolVec::with_capacity(argument_types.len() as u32, env.pool);

            for (type_id, loc_arg) in arguments.iter_node_ids().zip(argument_types.iter()) {
                as_type_id(
                    env,
                    scope,
                    references,
                    type_id,
                    &loc_arg.value,
                    loc_arg.region,
                );
            }

            let return_type_id = to_type_id(
                env,
                scope,
                references,
                &return_type.value,
                return_type.region,
            );

            let closure_type = Type2::Variable(env.var_store.fresh());
            let closure_type_id = env.pool.add(closure_type);

            Type2::Function(arguments, closure_type_id, return_type_id)
        }
        BoundVariable(v) => {
            // a rigid type variable
            match references.named.get(v) {
                Some(var) => Type2::Variable(*var),
                None => {
                    let var = env.var_store.fresh();

                    references.named.insert(v, var);

                    Type2::Variable(var)
                }
            }
        }
        Wildcard | Malformed(_) => {
            let var = env.var_store.fresh();

            references.unnamed.insert(var);

            Type2::Variable(var)
        }
        Record { fields, ext, .. } => {
            let field_types_map = can_assigned_fields(env, scope, references, fields, region);

            let field_types = PoolVec::with_capacity(field_types_map.len() as u32, env.pool);

            for (node_id, (label, field)) in field_types.iter_node_ids().zip(field_types_map) {
                let poolstr = PoolStr::new(label, env.pool);
                env.pool[node_id] = (poolstr, field);
            }

            let ext_type = match ext {
                Some(loc_ann) => to_type_id(env, scope, references, &loc_ann.value, region),
                None => env.add(Type2::EmptyRec, region),
            };

            Type2::Record(field_types, ext_type)
        }
        TagUnion { tags, ext, .. } => {
            let tag_types_vec = can_tags(env, scope, references, tags, region);

            let tag_types = PoolVec::with_capacity(tag_types_vec.len() as u32, env.pool);

            for (node_id, (label, field)) in tag_types.iter_node_ids().zip(tag_types_vec) {
                let poolstr = PoolStr::new(label, env.pool);
                env.pool[node_id] = (poolstr, field);
            }

            let ext_type = match ext {
                Some(loc_ann) => to_type_id(env, scope, references, &loc_ann.value, region),
                None => env.add(Type2::EmptyTagUnion, region),
            };

            Type2::TagUnion(tag_types, ext_type)
        }
        As(loc_inner, _spaces, loc_as) => {
            // e.g. `{ x : Int, y : Int } as Point }`
            match loc_as.value {
                Apply(module_name, ident, loc_vars) if module_name.is_empty() => {
                    let symbol = match scope.introduce(
                        ident.into(),
                        &env.exposed_ident_ids,
                        &mut env.ident_ids,
                        region,
                    ) {
                        Ok(symbol) => symbol,

                        Err((original_region, shadow)) => {
                            let problem = Problem::Shadowed(original_region, shadow.clone());

                            env.problem(roc_problem::can::Problem::ShadowingInAnnotation {
                                original_region,
                                shadow,
                            });

                            return Type2::Erroneous(problem);
                        }
                    };

                    let inner_type = to_type2(env, scope, references, &loc_inner.value, region);
                    let vars = PoolVec::with_capacity(loc_vars.len() as u32, env.pool);

                    let lowercase_vars = PoolVec::with_capacity(loc_vars.len() as u32, env.pool);

                    for ((loc_var, named_id), var_id) in loc_vars
                        .iter()
                        .zip(lowercase_vars.iter_node_ids())
                        .zip(vars.iter_node_ids())
                    {
                        match loc_var.value {
                            BoundVariable(ident) => {
                                let var_name = ident;

                                if let Some(var) = references.named.get(&var_name) {
                                    let poolstr = PoolStr::new(var_name, env.pool);

                                    let type_id = env.pool.add(Type2::Variable(*var));
                                    env.pool[var_id] = (poolstr.duplicate(), type_id);

                                    env.pool[named_id] = (poolstr, *var);
                                    env.set_region(named_id, loc_var.region);
                                } else {
                                    let var = env.var_store.fresh();

                                    references.named.insert(var_name.clone(), var);
                                    let poolstr = PoolStr::new(var_name, env.pool);

                                    let type_id = env.pool.add(Type2::Variable(var));
                                    env.pool[var_id] = (poolstr.duplicate(), type_id);

                                    env.pool[named_id] = (poolstr, var);
                                    env.set_region(named_id, loc_var.region);
                                }
                            }
                            _ => {
                                // If anything other than a lowercase identifier
                                // appears here, the whole annotation is invalid.
                                return Type2::Erroneous(Problem::CanonicalizationProblem);
                            }
                        }
                    }

                    let alias_actual = inner_type;
                    // TODO instantiate recursive tag union
                    //                    let alias_actual = if let Type2::TagUnion(tags, ext) = inner_type {
                    //                        let rec_var = env.var_store.fresh();
                    //
                    //                        let mut new_tags = Vec::with_capacity(tags.len());
                    //                        for (tag_name, args) in tags {
                    //                            let mut new_args = Vec::with_capacity(args.len());
                    //                            for arg in args {
                    //                                let mut new_arg = arg.clone();
                    //                                new_arg.substitute_alias(symbol, &Type2::Variable(rec_var));
                    //                                new_args.push(new_arg);
                    //                            }
                    //                            new_tags.push((tag_name.clone(), new_args));
                    //                        }
                    //                        Type2::RecursiveTagUnion(rec_var, new_tags, ext)
                    //                    } else {
                    //                        inner_type
                    //                    };

                    let mut hidden_variables = MutSet::default();
                    hidden_variables.extend(alias_actual.variables(env.pool));

                    for (_, var) in lowercase_vars.iter(env.pool) {
                        hidden_variables.remove(var);
                    }

                    let alias_actual_id = env.pool.add(alias_actual);
                    scope.add_alias(env.pool, symbol, lowercase_vars, alias_actual_id);

                    let alias = scope.lookup_alias(symbol).unwrap();
                    // local_aliases.insert(symbol, alias.clone());

                    // TODO host-exposed
                    //                    if vars.is_empty() && env.home == symbol.module_id() {
                    //                        let actual_var = env.var_store.fresh();
                    //                        rigids.host_exposed.insert(symbol, actual_var);
                    //                        Type::HostExposedAlias {
                    //                            name: symbol,
                    //                            arguments: vars,
                    //                            actual: Box::new(alias.typ.clone()),
                    //                            actual_var,
                    //                        }
                    //                    } else {
                    //                        Type::Alias(symbol, vars, Box::new(alias.typ.clone()))
                    //                    }
                    Type2::AsAlias(symbol, vars, alias.actual)
                }
                _ => {
                    // This is a syntactically invalid type alias.
                    Type2::Erroneous(Problem::CanonicalizationProblem)
                }
            }
        }
        SpaceBefore(nested, _) | SpaceAfter(nested, _) => {
            to_type2(env, scope, references, nested, region)
        }
    }
}

// TODO trim down these arguments!
#[allow(clippy::too_many_arguments)]
fn can_assigned_fields<'a>(
    env: &mut Env,
    scope: &mut Scope,
    rigids: &mut References<'a>,
    fields: &&[Located<roc_parse::ast::AssignedField<'a, roc_parse::ast::TypeAnnotation<'a>>>],
    region: Region,
) -> MutMap<&'a str, RecordField<Type2>> {
    use roc_parse::ast::AssignedField::*;
    use roc_types::types::RecordField::*;

    // SendMap doesn't have a `with_capacity`
    let mut field_types = MutMap::default();

    // field names we've seen so far in this record
    let mut seen = std::collections::HashMap::with_capacity(fields.len());

    'outer: for loc_field in fields.iter() {
        let mut field = &loc_field.value;

        // use this inner loop to unwrap the SpaceAfter/SpaceBefore
        // when we find the name of this field, break out of the loop
        // with that value, so we can check whether the field name is
        // a duplicate
        let new_name = 'inner: loop {
            match field {
                RequiredValue(field_name, _, annotation) => {
                    let field_type =
                        to_type2(env, scope, rigids, &annotation.value, annotation.region);

                    let label = field_name.value;
                    field_types.insert(label, Required(field_type));

                    break 'inner label;
                }
                OptionalValue(field_name, _, annotation) => {
                    let field_type =
                        to_type2(env, scope, rigids, &annotation.value, annotation.region);

                    let label = field_name.value;
                    field_types.insert(label.clone(), Optional(field_type));

                    break 'inner label;
                }
                LabelOnly(loc_field_name) => {
                    // Interpret { a, b } as { a : a, b : b }
                    let field_name = loc_field_name.value;
                    let field_type = {
                        if let Some(var) = rigids.named.get(&field_name) {
                            Type2::Variable(*var)
                        } else {
                            let field_var = env.var_store.fresh();
                            rigids.named.insert(field_name, field_var);
                            Type2::Variable(field_var)
                        }
                    };

                    field_types.insert(field_name.clone(), Required(field_type));

                    break 'inner field_name;
                }
                SpaceBefore(nested, _) | SpaceAfter(nested, _) => {
                    // check the nested field instead
                    field = nested;
                    continue 'inner;
                }
                Malformed(_) => {
                    // TODO report this?
                    // completely skip this element, advance to the next tag
                    continue 'outer;
                }
            }
        };

        // ensure that the new name is not already in this record:
        // note that the right-most tag wins when there are two with the same name
        if let Some(replaced_region) = seen.insert(new_name.clone(), loc_field.region) {
            env.problem(roc_problem::can::Problem::DuplicateRecordFieldType {
                field_name: new_name.into(),
                record_region: region,
                field_region: loc_field.region,
                replaced_region,
            });
        }
    }

    field_types
}

fn can_tags<'a>(
    env: &mut Env,
    scope: &mut Scope,
    rigids: &mut References<'a>,
    tags: &'a [Located<roc_parse::ast::Tag<'a>>],
    region: Region,
) -> Vec<(&'a str, PoolVec<Type2>)> {
    use roc_parse::ast::Tag;
    let mut tag_types = Vec::with_capacity(tags.len());

    // tag names we've seen so far in this tag union
    let mut seen = std::collections::HashMap::with_capacity(tags.len());

    'outer: for loc_tag in tags.iter() {
        let mut tag = &loc_tag.value;

        // use this inner loop to unwrap the SpaceAfter/SpaceBefore
        // when we find the name of this tag, break out of the loop
        // with that value, so we can check whether the tag name is
        // a duplicate
        let new_name = 'inner: loop {
            match tag {
                Tag::Global { name, args } => {
                    let arg_types = PoolVec::with_capacity(args.len() as u32, env.pool);

                    for (type_id, loc_arg) in arg_types.iter_node_ids().zip(args.iter()) {
                        as_type_id(env, scope, rigids, type_id, &loc_arg.value, loc_arg.region);
                    }

                    let tag_name = name.value;
                    tag_types.push((tag_name, arg_types));

                    break 'inner tag_name;
                }
                Tag::Private { name, args } => {
                    //let ident_id = env.ident_ids.get_or_insert(&name.value.into());
                    //let symbol = Symbol::new(env.home, ident_id);

                    let arg_types = PoolVec::with_capacity(args.len() as u32, env.pool);

                    for (type_id, loc_arg) in arg_types.iter_node_ids().zip(args.iter()) {
                        as_type_id(env, scope, rigids, type_id, &loc_arg.value, loc_arg.region);
                    }

                    //let tag_name = TagName::Private(symbol);
                    let tag_name = name.value;
                    tag_types.push((tag_name, arg_types));

                    break 'inner tag_name;
                }
                Tag::SpaceBefore(nested, _) | Tag::SpaceAfter(nested, _) => {
                    // check the nested tag instead
                    tag = nested;
                    continue 'inner;
                }
                Tag::Malformed(_) => {
                    // TODO report this?
                    // completely skip this element, advance to the next tag
                    continue 'outer;
                }
            }
        };

        // ensure that the new name is not already in this tag union:
        // note that the right-most tag wins when there are two with the same name
        if let Some(replaced_region) = seen.insert(new_name.clone(), loc_tag.region) {
            let tag_name = TagName::Global(new_name.into());
            env.problem(roc_problem::can::Problem::DuplicateTag {
                tag_region: loc_tag.region,
                tag_union_region: region,
                replaced_region,
                tag_name,
            });
        }
    }

    tag_types
}

enum TypeApply {
    Apply(Symbol, PoolVec<Type2>),
    Alias(Symbol, PoolVec<(PoolStr, TypeId)>, TypeId),
    Erroneous(roc_types::types::Problem),
}

#[inline(always)]
fn to_type_apply<'a>(
    env: &mut Env,
    scope: &mut Scope,
    rigids: &mut References<'a>,
    module_name: &str,
    ident: &str,
    type_arguments: &[Located<roc_parse::ast::TypeAnnotation<'a>>],
    region: Region,
) -> TypeApply {
    let symbol = if module_name.is_empty() {
        // Since module_name was empty, this is an unqualified type.
        // Look it up in scope!
        let ident: Ident = (*ident).into();

        match scope.lookup(&ident, region) {
            Ok(symbol) => symbol,
            Err(problem) => {
                env.problem(roc_problem::can::Problem::RuntimeError(problem));

                return TypeApply::Erroneous(Problem::UnrecognizedIdent(ident.into()));
            }
        }
    } else {
        match env.qualified_lookup(module_name, ident, region) {
            Ok(symbol) => symbol,
            Err(problem) => {
                // Either the module wasn't imported, or
                // it was imported but it doesn't expose this ident.
                env.problem(roc_problem::can::Problem::RuntimeError(problem));

                return TypeApply::Erroneous(Problem::UnrecognizedIdent((*ident).into()));
            }
        }
    };

    let argument_type_ids = PoolVec::with_capacity(type_arguments.len() as u32, env.pool);

    for (type_id, loc_arg) in argument_type_ids.iter_node_ids().zip(type_arguments.iter()) {
        as_type_id(env, scope, rigids, type_id, &loc_arg.value, loc_arg.region);
    }

    let args = type_arguments;
    let opt_alias = scope.lookup_alias(symbol);
    match opt_alias {
        Some(ref alias) => {
            // use a known alias
            let actual = alias.actual;
            let mut substitutions: MutMap<Variable, TypeId> = MutMap::default();

            if alias.targs.len() != args.len() {
                let error = TypeApply::Erroneous(Problem::BadTypeArguments {
                    symbol,
                    region,
                    alias_needs: alias.targs.len() as u8,
                    type_got: args.len() as u8,
                });
                return error;
            }

            let arguments = PoolVec::with_capacity(type_arguments.len() as u32, env.pool);

            let it = arguments.iter_node_ids().zip(
                argument_type_ids
                    .iter_node_ids()
                    .zip(alias.targs.iter_node_ids()),
            );

            for (node_id, (type_id, loc_var_id)) in it {
                let loc_var = &env.pool[loc_var_id];
                let name = loc_var.0.duplicate();
                let var = loc_var.1;

                env.pool[node_id] = (name, type_id);

                substitutions.insert(var, type_id);
            }

            // make sure the recursion variable is freshly instantiated
            // have to allocate these outside of the if for lifetime reasons...
            let new = env.var_store.fresh();
            let fresh = env.pool.add(Type2::Variable(new));
            if let Type2::RecursiveTagUnion(rvar, ref tags, ext) = &mut env.pool[actual] {
                substitutions.insert(*rvar, fresh);

                env.pool[actual] = Type2::RecursiveTagUnion(new, tags.duplicate(), *ext);
            }

            // make sure hidden variables are freshly instantiated
            for var_id in alias.hidden_variables.iter_node_ids() {
                let var = env.pool[var_id];
                let fresh = env.pool.add(Type2::Variable(env.var_store.fresh()));
                substitutions.insert(var, fresh);
            }

            // instantiate variables
            Type2::substitute(env.pool, &substitutions, actual);

            TypeApply::Alias(symbol, arguments, actual)
        }
        None => TypeApply::Apply(symbol, argument_type_ids),
    }
}

#[derive(Debug)]
pub struct Alias {
    pub targs: PoolVec<(PoolStr, Variable)>,
    pub actual: TypeId,

    /// hidden type variables, like the closure variable in `a -> b`
    pub hidden_variables: PoolVec<Variable>,
}