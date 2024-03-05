// Copyright 2024 The Jujutsu Authors
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// https://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use jj_cli::cli_util::CliRunner;
use jj_cli::commit_templater::{CommitTemplateBuildFnTable, CommitTemplateLanguageExtension};
use jj_cli::template_builder::TemplateLanguage;
use jj_cli::template_parser::{self, TemplateParseError};
use jj_cli::templater::{TemplateFunction, TemplatePropertyError};
use jj_lib::backend::CommitId;
use jj_lib::commit::Commit;
use jj_lib::object_id::ObjectId;
use jj_lib::repo::Repo;
use jj_lib::revset::RevsetExpression;

struct HexCounter;

fn num_digits_in_id(id: &CommitId) -> i64 {
    let mut count = 0;
    for ch in id.hex().chars() {
        if ch.is_ascii_digit() {
            count += 1;
        }
    }
    count
}

fn num_char_in_id(commit: Commit, ch_match: char) -> Result<i64, TemplatePropertyError> {
    let mut count = 0;
    for ch in commit.id().hex().chars() {
        if ch == ch_match {
            count += 1;
        }
    }
    Ok(count)
}

#[derive(Default)]
struct MostDigitsInId {
    count: i64,
}

impl MostDigitsInId {
    fn new(repo: &dyn Repo) -> Self {
        let count = RevsetExpression::all()
            .evaluate_programmatic(repo)
            .unwrap()
            .iter()
            .map(|id| num_digits_in_id(&id))
            .max()
            .unwrap_or(0);
        Self { count }
    }

    fn count(&self) -> i64 {
        self.count
    }
}

impl CommitTemplateLanguageExtension for HexCounter {
    fn build_fn_table<'repo>(&self) -> CommitTemplateBuildFnTable<'repo> {
        let mut table = CommitTemplateBuildFnTable::empty();
        table.commit_methods.insert(
            "has_most_digits",
            |language, _build_context, property, call| {
                template_parser::expect_no_arguments(call)?;
                let most_digits = language
                    .cache_extension(|| MostDigitsInId::new(language.repo()))
                    .count();
                Ok(
                    language.wrap_boolean(TemplateFunction::new(property, move |commit| {
                        Ok(num_digits_in_id(commit.id()) == most_digits)
                    })),
                )
            },
        );
        table.commit_methods.insert(
            "num_digits_in_id",
            |language, _build_context, property, call| {
                template_parser::expect_no_arguments(call)?;
                Ok(
                    language.wrap_integer(TemplateFunction::new(property, |commit| {
                        Ok(num_digits_in_id(commit.id()))
                    })),
                )
            },
        );
        table.commit_methods.insert(
            "num_char_in_id",
            |language, _build_context, property, call| {
                let [string_arg] = template_parser::expect_exact_arguments(call)?;
                let char_arg =
                    template_parser::expect_string_literal_with(string_arg, |string, span| {
                        let chars: Vec<_> = string.chars().collect();
                        match chars[..] {
                            [ch] => Ok(ch),
                            _ => Err(TemplateParseError::unexpected_expression(
                                "Expected singular character argument",
                                span,
                            )),
                        }
                    })?;

                Ok(
                    language.wrap_integer(TemplateFunction::new(property, move |commit| {
                        num_char_in_id(commit, char_arg)
                    })),
                )
            },
        );

        table
    }
}

fn main() -> std::process::ExitCode {
    CliRunner::init()
        .set_commit_template_extension(Box::new(HexCounter))
        .run()
}
