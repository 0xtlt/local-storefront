//! What a bundle needs of each module: the exports read, the statements kept, the modules that
//! run (tree shaking).

use std::collections::{BTreeSet, VecDeque};

use super::*;

/// What the bundle needs of each module.
pub(super) struct Live {
    /// The modules that run.
    pub(super) needed: Vec<bool>,
    /// The statements kept, by module.
    pub(super) statements: Vec<Vec<bool>>,
    /// The exports read, by module.
    pub(super) exports: Vec<BTreeSet<String>>,
    /// The modules whose every export is read (a namespace, a `require`).
    pub(super) whole: Vec<bool>,
}

pub(super) enum Work {
    Export(usize, String),
    Whole(usize),
    Name(usize, String),
    Statement(usize, usize),
}

impl Graph<'_> {
    /// What the Function needs: from its `roots` (module, export) and the modules that always
    /// run (`entries`), the exports read, the statements kept, the modules that run.
    pub(super) fn live(&self, roots: &[(usize, String)], entries: &[usize]) -> Live {
        let count = self.modules.len();
        let mut live = Live {
            needed: vec![false; count],
            statements: self
                .modules
                .iter()
                .map(|module| vec![false; module.statements.len()])
                .collect(),
            exports: vec![BTreeSet::new(); count],
            whole: vec![false; count],
        };
        let mut work: VecDeque<Work> = VecDeque::new();
        for (id, module) in self.modules.iter().enumerate() {
            if module.form == Form::CommonJs {
                work.push_back(Work::Whole(id));
            }
            for (index, statement) in module.statements.iter().enumerate() {
                if statement.effects {
                    work.push_back(Work::Statement(id, index));
                }
            }
        }
        for (id, name) in roots {
            work.push_back(Work::Export(*id, name.clone()));
        }
        while let Some(item) = work.pop_front() {
            match item {
                Work::Export(id, name) => {
                    let module = &self.modules[id];
                    if module.form != Form::Esm {
                        work.push_back(Work::Whole(id));
                        continue;
                    }
                    if !live.exports[id].insert(name.clone()) {
                        continue;
                    }
                    match module.exports.get(&name) {
                        Some(Export::Local(local)) => {
                            work.push_back(Work::Name(id, local.clone()));
                        }
                        Some(Export::From(from, imported)) => {
                            work.push_back(Work::Export(*from, imported.clone()));
                        }
                        Some(Export::Namespace(from)) => work.push_back(Work::Whole(*from)),
                        None => {
                            for &star in &module.stars {
                                if self
                                    .exports_of(star)
                                    .is_none_or(|names| names.contains(&name))
                                {
                                    work.push_back(Work::Export(star, name.clone()));
                                }
                            }
                        }
                    }
                }
                Work::Whole(id) => {
                    if live.whole[id] {
                        continue;
                    }
                    live.whole[id] = true;
                    let module = &self.modules[id];
                    for name in module.exports.keys() {
                        work.push_back(Work::Export(id, name.clone()));
                    }
                    for &star in &module.stars {
                        work.push_back(Work::Whole(star));
                    }
                    for &required in module.requires.values() {
                        work.push_back(Work::Whole(required));
                    }
                }
                Work::Name(id, local) => {
                    let module = &self.modules[id];
                    if let Some(import) = module.imports.get(&local) {
                        work.push_back(match &import.name {
                            Some(name) => Work::Export(import.from, name.clone()),
                            None => Work::Whole(import.from),
                        });
                    } else if let Some(statements) = module.declared_by.get(&local) {
                        for &index in statements {
                            work.push_back(Work::Statement(id, index));
                        }
                    }
                }
                Work::Statement(id, index) => {
                    if live.statements[id][index] {
                        continue;
                    }
                    live.statements[id][index] = true;
                    let module = &self.modules[id];
                    let statement = &module.statements[index];
                    for used in &statement.uses {
                        match used {
                            Use::Name(name) => work.push_back(Work::Name(id, name.clone())),
                            Use::Member(name, property) => {
                                if self.enum_member(id, name, property).is_some() {
                                    continue;
                                }
                                match module.imports.get(name) {
                                    Some(Import { from, name: None }) => {
                                        work.push_back(Work::Export(*from, property.clone()));
                                    }
                                    _ => work.push_back(Work::Name(id, name.clone())),
                                }
                            }
                        }
                    }
                    for &(_, required) in &statement.requires {
                        work.push_back(Work::Whole(required));
                    }
                }
            }
        }

        // A module runs when something of it is kept, or when it is the way to a module that
        // runs (its import runs that module, in order).
        for id in 0..count {
            live.needed[id] = live.whole[id]
                || !live.exports[id].is_empty()
                || live.statements[id].iter().any(|&kept| kept);
        }
        for &entry in entries {
            live.needed[entry] = true;
        }
        loop {
            let mut changed = false;
            for id in 0..count {
                if !live.needed[id]
                    && self.modules[id]
                        .requests
                        .iter()
                        .any(|&(_, request)| live.needed[request])
                {
                    live.needed[id] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        live
    }
}
