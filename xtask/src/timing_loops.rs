//! Rule (d): short wall-clock exits in sleep/park/yield loops. Keep the original line-based
//! checks separate. Syntax gives us loop boundaries, lexical bindings and multiline conditions
//! without confusing a polling interval with its deadline, or strings with executable code.
//! This is a same-file check, not interprocedural analysis of clocks hidden in other functions.

use super::Hit;
use std::collections::{HashMap, HashSet};
use syn::{
    Expr, Item, Pat, Stmt,
    visit::{self, Visit},
};

#[derive(Clone, Copy, Default)]
struct Value {
    short: bool,
    wall: bool,
    number: Option<u64>,
    nanos: Option<u64>,
    instant: bool,
    fine: bool,
    injected: bool,
    elapsed_limit: Option<f64>,
}

impl Value {
    fn merge(&mut self, other: Self) {
        self.short |= other.short;
        self.wall |= other.wall;
        self.fine |= other.fine;
        self.injected |= other.injected;
    }
}

type Bindings = HashMap<String, Value>;

fn lookup<'a>(bindings: &'a Bindings, name: &str) -> Option<&'a Value> {
    bindings.get(name.strip_prefix("self::").unwrap_or(name))
}

fn scoped(bindings: &Bindings, name: &str) -> Bindings {
    let mut out = bindings.clone();
    for (key, v) in bindings {
        out.insert(format!("super::{key}"), *v);
        if let Some(local) = key.strip_prefix(&format!("{name}::")) {
            out.insert(local.into(), *v);
            out.insert(format!("Self::{local}"), *v);
        }
    }
    out
}

fn path_name(path: &syn::Path) -> String {
    path.segments.iter().map(|s| s.ident.to_string()).collect::<Vec<_>>().join("::")
}

fn value(expr: &Expr, bindings: &Bindings) -> Value {
    struct Read<'a> {
        bindings: &'a Bindings,
        out: Value,
    }
    impl<'ast> Visit<'ast> for Read<'_> {
        fn visit_expr_path(&mut self, e: &'ast syn::ExprPath) {
            if let Some(v) = lookup(self.bindings, &path_name(&e.path)) {
                self.out.merge(*v);
            }
        }
        fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
            if let Expr::Path(p) = e.func.as_ref() {
                let name = path_name(&p.path);
                self.out.wall |= name.ends_with("Instant::now");
                self.out.injected |= lookup(self.bindings, &name).is_some_and(|v| v.injected);
                if let Some((_, unit)) = name.rsplit_once("Duration::from_") {
                    let n = e.args.first().and_then(|arg| number(arg, self.bindings));
                    self.out.fine |= matches!(unit, "micros" | "nanos");
                    self.out.short |= match unit {
                        "millis" => n.is_some_and(|n| n < 250),
                        "micros" | "nanos" => true,
                        _ => false,
                    };
                }
            }
            // A duration passed to a constructor/helper is not that call's return value.
            // Function-scope short arguments are checked separately from return values.
        }
        fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
            // Even an injected Instant's .elapsed() reads the real clock again.
            self.out.wall |= e.method == "elapsed";
            self.out.injected |= e.method == "now";
            visit::visit_expr_method_call(self, e);
        }
        fn visit_expr_binary(&mut self, e: &'ast syn::ExprBinary) {
            let (l, r) = (value(&e.left, self.bindings), value(&e.right, self.bindings));
            self.out.wall |= l.wall || r.wall;
            self.out.injected |= l.injected || r.injected;
            if matches!(e.op, syn::BinOp::Lt(_) | syn::BinOp::Le(_) | syn::BinOp::Gt(_) | syn::BinOp::Ge(_) | syn::BinOp::Eq(_) | syn::BinOp::Ne(_)) {
                self.out.short |= short_numeric_bound(l.elapsed_limit, &e.right) || short_numeric_bound(r.elapsed_limit, &e.left);
            }
            // Instant subtraction is an observed age, not a literal zero-length budget.
            if matches!(e.op, syn::BinOp::Sub(_)) && l.instant && r.instant {
                return;
            }
            let nanos = match e.op {
                syn::BinOp::Add(_) => l.nanos.zip(r.nanos).and_then(|(l, r)| l.checked_add(r)),
                syn::BinOp::Sub(_) => l.nanos.zip(r.nanos).and_then(|(l, r)| l.checked_sub(r)),
                syn::BinOp::Mul(_) => l.nanos.zip(r.number).or_else(|| r.nanos.zip(l.number)).and_then(|(l, r)| l.checked_mul(r)),
                syn::BinOp::Div(_) => l.nanos.zip(r.number).and_then(|(l, r)| l.checked_div(r)),
                _ => None,
            };
            self.out.fine |= l.fine || r.fine;
            self.out.short |= self.out.fine || nanos.map_or(l.short || r.short, |n| n < 250_000_000);
        }
        // A stored clock closure does not read the wall clock merely by being constructed.
        fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    }
    let mut read = Read { bindings, out: Value::default() };
    read.visit_expr(expr);
    read.out.number = number(expr, bindings);
    read.out.nanos = duration(expr, bindings);
    read.out.elapsed_limit = match expr {
        Expr::Path(p) => lookup(bindings, &path_name(&p.path)).and_then(|v| v.elapsed_limit),
        Expr::Paren(e) => value(&e.expr, bindings).elapsed_limit,
        Expr::MethodCall(e) if value(&e.receiver, bindings).wall => match e.method.to_string().as_str() {
            "as_millis" => Some(250.0),
            "as_micros" => Some(250_000.0),
            "as_secs_f64" => Some(0.25),
            _ => None,
        },
        _ => None,
    };
    read.out.instant = match expr {
        Expr::Path(p) => lookup(bindings, &path_name(&p.path)).is_some_and(|v| v.instant),
        Expr::Call(e) => matches!(e.func.as_ref(), Expr::Path(p) if path_name(&p.path).ends_with("Instant::now")),
        Expr::MethodCall(e) => e.method == "now",
        Expr::Paren(e) => value(&e.expr, bindings).instant,
        Expr::Binary(e) if matches!(e.op, syn::BinOp::Add(_) | syn::BinOp::Sub(_)) => {
            value(&e.left, bindings).instant && !value(&e.right, bindings).instant
        }
        _ => false,
    };
    read.out
}

fn short_numeric_bound(limit: Option<f64>, expr: &Expr) -> bool {
    let Some(limit) = limit else { return false };
    match expr {
        Expr::Lit(e) => match &e.lit {
            syn::Lit::Int(n) => n.base10_parse::<f64>().is_ok_and(|n| n < limit),
            syn::Lit::Float(n) => n.base10_parse::<f64>().is_ok_and(|n| n < limit),
            _ => false,
        },
        Expr::Paren(e) => short_numeric_bound(Some(limit), &e.expr),
        _ => false,
    }
}

fn duration(expr: &Expr, bindings: &Bindings) -> Option<u64> {
    match expr {
        Expr::Path(p) => lookup(bindings, &path_name(&p.path)).and_then(|v| v.nanos),
        Expr::Paren(e) => duration(&e.expr, bindings),
        Expr::Call(e) => {
            let Expr::Path(p) = e.func.as_ref() else { return None };
            let name = path_name(&p.path);
            if name.ends_with("Instant::now") {
                return Some(0);
            }
            let unit = name.rsplit_once("Duration::from_")?.1;
            let scale = match unit {
                "secs" => 1_000_000_000,
                "millis" => 1_000_000,
                "micros" => 1_000,
                "nanos" => 1,
                _ => return None,
            };
            number(e.args.first()?, bindings)?.checked_mul(scale)
        }
        Expr::Binary(e) => match e.op {
            syn::BinOp::Add(_) => duration(&e.left, bindings)?.checked_add(duration(&e.right, bindings)?),
            syn::BinOp::Sub(_) => {
                if value(&e.left, bindings).instant && value(&e.right, bindings).instant {
                    return None;
                }
                duration(&e.left, bindings)?.checked_sub(duration(&e.right, bindings)?)
            }
            syn::BinOp::Mul(_) => duration(&e.left, bindings)?.checked_mul(number(&e.right, bindings)?),
            syn::BinOp::Div(_) => duration(&e.left, bindings)?.checked_div(number(&e.right, bindings)?),
            _ => None,
        },
        _ => None,
    }
}

fn number(expr: &Expr, bindings: &Bindings) -> Option<u64> {
    match expr {
        Expr::Lit(e) => {
            if let syn::Lit::Int(n) = &e.lit {
                n.base10_parse().ok()
            } else {
                None
            }
        }
        Expr::Path(e) => lookup(bindings, &path_name(&e.path)).and_then(|v| v.number),
        Expr::Paren(e) => number(&e.expr, bindings),
        Expr::Binary(e) => {
            let (l, r) = (number(&e.left, bindings)?, number(&e.right, bindings)?);
            match e.op {
                syn::BinOp::Add(_) => l.checked_add(r),
                syn::BinOp::Sub(_) => l.checked_sub(r),
                syn::BinOp::Mul(_) => l.checked_mul(r),
                syn::BinOp::Div(_) => l.checked_div(r),
                _ => None,
            }
        }
        _ => None,
    }
}

fn constants<'a>(items: impl Iterator<Item = &'a Item>, bindings: &mut Bindings, root: bool) {
    fn collect<'a>(items: impl Iterator<Item = &'a Item>, prefix: &str, defs: &mut Vec<(String, &'a Expr)>) {
        for item in items {
            match item {
                Item::Const(c) => defs.push((format!("{prefix}{}", c.ident), &c.expr)),
                Item::Static(c) => defs.push((format!("{prefix}{}", c.ident), &c.expr)),
                Item::Mod(m) => {
                    if let Some((_, items)) = &m.content {
                        collect(items.iter(), &format!("{prefix}{}::", m.ident), defs);
                    }
                }
                Item::Impl(i) => {
                    if let syn::Type::Path(p) = i.self_ty.as_ref() {
                        for member in &i.items {
                            if let syn::ImplItem::Const(c) = member {
                                defs.push((format!("{prefix}{}::{}", path_name(&p.path), c.ident), &c.expr));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
    let mut defs = Vec::new();
    collect(items, "", &mut defs);
    // Rust permits forward references. At most one pass per definition resolves any acyclic
    // chain; cycles are rejected by rustc, and cannot make this gate hang.
    for _ in 0..defs.len() {
        for (name, expr) in &defs {
            let mut context = bindings.clone();
            if let Some((namespace, _)) = name.rsplit_once("::") {
                for part in namespace.split("::") {
                    context = scoped(&context, part);
                }
            }
            let v = value(expr, &context);
            bindings.insert(name.clone(), v);
            if root {
                bindings.insert(format!("crate::{name}"), v);
            }
        }
    }
}

fn bind(pat: &Pat, v: Value, bindings: &mut Bindings) {
    match pat {
        Pat::Ident(p) => {
            bindings.insert(p.ident.to_string(), v);
        }
        Pat::Type(p) => bind(&p.pat, v, bindings),
        _ => {}
    }
}

#[derive(Default)]
struct PollLoop {
    paced: bool,
    timed: bool,
    injected: bool,
    condition_names: HashSet<String>,
}

/// Exit detection stops at nested loops and closures: their breaks do not end this loop.
struct Exit<'a> {
    found: bool,
    condition_names: Option<&'a HashSet<String>>,
}

impl<'ast> Visit<'ast> for Exit<'_> {
    fn visit_expr_break(&mut self, _: &'ast syn::ExprBreak) {
        self.found = true;
    }
    fn visit_expr_return(&mut self, _: &'ast syn::ExprReturn) {
        self.found = true;
    }
    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        self.found |= m.path.segments.last().is_some_and(|s| s.ident == "panic" || s.ident == "unreachable");
    }
    fn visit_expr_assign(&mut self, e: &'ast syn::ExprAssign) {
        if let Expr::Path(p) = e.left.as_ref() {
            self.found |= self.condition_names.is_some_and(|names| names.contains(&path_name(&p.path)));
        }
        visit::visit_expr_assign(self, e);
    }
    fn visit_item(&mut self, _: &'ast Item) {}
    fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
    fn visit_expr_loop(&mut self, _: &'ast syn::ExprLoop) {}
    fn visit_expr_while(&mut self, _: &'ast syn::ExprWhile) {}
    fn visit_expr_for_loop(&mut self, _: &'ast syn::ExprForLoop) {}
}

#[derive(Default)]
struct Function {
    short_expiry: bool,
    loops: Vec<(usize, PollLoop)>,
}

struct Scanner<'a> {
    path: &'a str,
    lines: Vec<&'a str>,
    in_test: bool,
    bindings: Bindings,
    loops: Vec<PollLoop>,
    function: Function,
    hits: Vec<Hit>,
}

impl Scanner<'_> {
    fn block(&mut self, b: &syn::Block) -> Bindings {
        let saved = self.bindings.clone();
        constants(b.stmts.iter().filter_map(|s| if let Stmt::Item(i) = s { Some(i) } else { None }), &mut self.bindings, false);
        visit::visit_block(self, b);
        std::mem::replace(&mut self.bindings, saved)
    }

    fn condition(&mut self, expr: &Expr) {
        let v = value(expr, &self.bindings);
        if v.short
            && v.wall
            && let Some(l) = self.loops.last_mut()
        {
            l.timed = true;
        }
    }

    fn marked(&self, line: usize) -> bool {
        let marker = |s: &str| {
            let (_, comment) = super::split_line(s);
            comment.strip_prefix("// timing:").is_some_and(|c| {
                let mut words = c.split_whitespace();
                words.next() == Some("stall-bound") && words.next().is_some()
            })
        };
        marker(self.lines[line - 1]) || (line > 1 && self.lines[line - 2].trim_start().starts_with("//") && marker(self.lines[line - 2]))
    }

    fn poll_loop(&mut self, line: usize, cond: Option<&Expr>, body: &syn::Block, repeats_condition: bool) {
        self.loops.push(PollLoop::default());
        if let Some(cond) = cond {
            struct Names(HashSet<String>);
            impl<'ast> Visit<'ast> for Names {
                fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
                    self.0.insert(path_name(&p.path));
                }
                fn visit_expr_closure(&mut self, _: &'ast syn::ExprClosure) {}
            }
            let mut names = Names(HashSet::new());
            names.visit_expr(cond);
            if repeats_condition && let Some(l) = self.loops.last_mut() {
                l.condition_names = names.0;
            }
            self.condition(cond);
            self.visit_expr(cond);
        }
        let after = self.block(body);
        if let Some(cond) = cond {
            let v = value(cond, &after);
            if v.short
                && v.wall
                && let Some(l) = self.loops.last_mut()
            {
                l.timed = true;
            }
        }
        if let Some(l) = self.loops.pop() {
            self.function.loops.push((line, l));
        }
    }

    fn finish_function(&mut self, outer: Function) {
        let function = std::mem::replace(&mut self.function, outer);
        for (line, l) in function.loops {
            if self.in_test && l.paced && (l.timed || (function.short_expiry && !l.injected)) && !self.marked(line) {
                self.hits.push(Hit {
                    path: self.path.into(),
                    line,
                    rule: 'd',
                    text: "sleep/park/yield loop may depend on a short wall-clock expiry; inject the clock or use a documented stall budget".into(),
                });
            }
        }
    }

    fn call_arguments<'a>(&mut self, args: impl Iterator<Item = &'a Expr>) {
        for arg in args {
            let v = value(arg, &self.bindings);
            self.function.short_expiry |= !v.instant && v.nanos.is_some_and(|n| n < 250_000_000);
            if v.injected && !v.wall {
                for l in &mut self.loops {
                    l.injected = true;
                }
            }
        }
    }

    fn function(&mut self, sig: &syn::Signature, body: &syn::Block) {
        let saved = self.bindings.clone();
        let outer_loops = std::mem::take(&mut self.loops);
        let outer_function = std::mem::take(&mut self.function);
        for arg in &sig.inputs {
            if let syn::FnArg::Typed(arg) = arg {
                bind_parameter(&arg.pat, &mut self.bindings);
            }
        }
        self.visit_block(body);
        self.finish_function(outer_function);
        self.bindings = saved;
        self.loops = outer_loops;
    }
}

fn bind_parameter(pat: &Pat, bindings: &mut Bindings) {
    let injected = match pat {
        Pat::Ident(p) => p.ident == "now",
        Pat::Type(p) => {
            bind_parameter(&p.pat, bindings);
            return;
        }
        _ => false,
    };
    bind(pat, Value { injected, ..Value::default() }, bindings);
}

impl<'ast> Visit<'ast> for Scanner<'_> {
    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        let was_test = self.in_test;
        self.in_test |= m.attrs.iter().any(|a| a.path().is_ident("cfg") && a.parse_args::<syn::Path>().is_ok_and(|p| p.is_ident("test")));
        let saved = self.bindings.clone();
        self.bindings = scoped(&saved, &m.ident.to_string());
        if let Some((_, items)) = &m.content {
            constants(items.iter(), &mut self.bindings, false);
            for item in items {
                self.visit_item(item);
            }
        }
        self.bindings = saved;
        self.in_test = was_test;
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        self.function(&f.sig, &f.block);
    }
    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        self.function(&f.sig, &f.block);
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        let saved = self.bindings.clone();
        if let syn::Type::Path(p) = i.self_ty.as_ref() {
            self.bindings = scoped(&saved, &path_name(&p.path));
        }
        visit::visit_item_impl(self, i);
        self.bindings = saved;
    }

    fn visit_block(&mut self, b: &'ast syn::Block) {
        self.block(b);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        visit::visit_local(self, l);
        let mut v = l.init.as_ref().map_or(Value::default(), |i| value(&i.expr, &self.bindings));
        // A real Instant captured before polling can be the origin of a manual clock.
        // Only reads performed during polling pace the exit; .elapsed() still counts.
        if self.loops.is_empty() {
            v.wall = false;
            v.injected |= v.instant;
        }
        bind(&l.pat, v, &mut self.bindings);
    }

    fn visit_expr_assign(&mut self, e: &'ast syn::ExprAssign) {
        if let Expr::Path(p) = e.left.as_ref()
            && self.loops.last().is_some_and(|l| l.condition_names.contains(&path_name(&p.path)))
        {
            self.condition(&e.right);
        }
        visit::visit_expr_assign(self, e);
        if let Expr::Path(p) = e.left.as_ref() {
            self.bindings.insert(path_name(&p.path), value(&e.right, &self.bindings));
        }
    }

    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        self.poll_loop(e.while_token.span.start().line, Some(&e.cond), &e.body, true);
    }
    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.poll_loop(e.for_token.span.start().line, Some(&e.expr), &e.body, false);
    }
    fn visit_expr_loop(&mut self, e: &'ast syn::ExprLoop) {
        self.poll_loop(e.loop_token.span.start().line, None, &e.body, false);
    }

    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        // Only exit-controlling branches count. A clock sampled for diagnostics must not
        // turn an unrelated state-driven polling loop into a short timeout.
        let mut exits = Exit { found: false, condition_names: self.loops.last().map(|l| &l.condition_names) };
        exits.visit_block(&e.then_branch);
        if let Some((_, other)) = &e.else_branch {
            exits.visit_expr(other);
        }
        if exits.found {
            self.condition(&e.cond);
        }
        visit::visit_expr_if(self, e);
    }

    fn visit_expr_match(&mut self, e: &'ast syn::ExprMatch) {
        fn overlaps(a: &Pat, b: &Pat) -> bool {
            match (a, b) {
                (Pat::Wild(_), _) | (_, Pat::Wild(_)) => true,
                (Pat::Ident(a), Pat::Ident(b)) => a.ident == b.ident,
                (Pat::Path(a), Pat::Path(b)) => path_name(&a.path) == path_name(&b.path),
                (Pat::Or(a), b) => a.cases.iter().any(|a| overlaps(a, b)),
                (a, Pat::Or(b)) => b.cases.iter().any(|b| overlaps(a, b)),
                _ => false,
            }
        }
        let exits: Vec<_> = e
            .arms
            .iter()
            .map(|arm| {
                let mut exit = Exit { found: false, condition_names: self.loops.last().map(|l| &l.condition_names) };
                exit.visit_expr(&arm.body);
                exit.found
            })
            .collect();
        if exits.iter().any(|exit| *exit) {
            self.condition(&e.expr);
            for (i, arm) in e.arms.iter().enumerate() {
                // A guard can select an exit in its own arm or a fallback for that pattern.
                // An unrelated Ready => break must not taint a Waiting diagnostic guard.
                let fallback_exits = e.arms[i + 1..].iter().zip(&exits[i + 1..]).any(|(other, exit)| *exit && overlaps(&arm.pat, &other.pat));
                if (exits[i] || fallback_exits)
                    && let Some((_, guard)) = &arm.guard
                {
                    self.condition(guard);
                }
            }
        }
        visit::visit_expr_match(self, e);
    }

    fn visit_macro(&mut self, m: &'ast syn::Macro) {
        if m.path.is_ident("assert") || m.path.is_ident("debug_assert") {
            use syn::parse::Parser;
            if let Ok(args) = syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated.parse2(m.tokens.clone())
                && let Some(cond) = args.first()
            {
                self.condition(cond);
            }
        }
    }

    fn visit_expr_call(&mut self, e: &'ast syn::ExprCall) {
        if let Expr::Path(p) = e.func.as_ref()
            && p.path.segments.last().is_some_and(|s| matches!(s.ident.to_string().as_str(), "sleep" | "park_timeout" | "yield_now"))
        {
            for l in &mut self.loops {
                l.paced = true;
            }
        } else {
            self.call_arguments(e.args.iter());
            if matches!(e.func.as_ref(), Expr::Path(p) if lookup(&self.bindings, &path_name(&p.path)).is_some_and(|v| v.injected)) {
                for l in &mut self.loops {
                    l.injected = true;
                }
            }
        }
        visit::visit_expr_call(self, e);
    }

    fn visit_expr_method_call(&mut self, e: &'ast syn::ExprMethodCall) {
        self.call_arguments(e.args.iter());
        if e.method == "now" {
            for l in &mut self.loops {
                l.injected = true;
            }
        }
        visit::visit_expr_method_call(self, e);
    }

    fn visit_expr_closure(&mut self, e: &'ast syn::ExprClosure) {
        let outer_loops = std::mem::take(&mut self.loops);
        let outer_function = std::mem::take(&mut self.function);
        let saved = self.bindings.clone();
        for input in &e.inputs {
            bind_parameter(input, &mut self.bindings);
        }
        visit::visit_expr_closure(self, e);
        self.finish_function(outer_function);
        self.bindings = saved;
        self.loops = outer_loops;
    }
}

pub(super) fn scan(path: &str, src: &str, test_file: bool) -> Vec<Hit> {
    let file = match syn::parse_file(src) {
        Ok(file) => file,
        Err(e) => return vec![Hit { path: path.into(), line: e.span().start().line, rule: 'd', text: format!("cannot check polling loops: {e}") }],
    };
    let mut scanner = Scanner {
        path,
        lines: src.lines().collect(),
        in_test: test_file,
        bindings: Bindings::new(),
        loops: Vec::new(),
        function: Function::default(),
        hits: Vec::new(),
    };
    constants(file.items.iter(), &mut scanner.bindings, true);
    scanner.visit_file(&file);
    scanner.hits.sort_by_key(|h| h.line);
    scanner.hits
}
