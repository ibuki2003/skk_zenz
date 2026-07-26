use anyhow::{Result, bail};

#[derive(Debug)]
pub(crate) enum Command {
    Context(String),
    Input(String),
    Eval(String),
}

#[derive(Debug)]
pub(crate) struct Operation {
    pub(crate) line: usize,
    pub(crate) command: Command,
}

impl Operation {
    pub(crate) fn label(&self) -> String {
        match &self.command {
            Command::Context(value) => format!("context {value}"),
            Command::Input(value) => format!("input {value}"),
            Command::Eval(value) => format!("eval {value}"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct EvalGroup {
    pub(crate) line: usize,
    pub(crate) context: String,
    pub(crate) input: String,
    pub(crate) outputs: Vec<String>,
}

pub(crate) fn parse_script(contents: &str) -> Result<Vec<Operation>> {
    let mut operations = Vec::new();
    for (index, line) in contents.lines().enumerate() {
        let line_number = index + 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, argument)) = line.split_once(' ') else {
            bail!("{line_number}行目: コマンドと引数を空白で区切ってください");
        };
        let command = match name {
            "context" => Command::Context(argument.to_owned()),
            "input" => Command::Input(argument.to_owned()),
            "eval" => Command::Eval(argument.to_owned()),
            _ => bail!("{line_number}行目: 未知のコマンドです: {name}"),
        };
        operations.push(Operation {
            line: line_number,
            command,
        });
    }
    if operations.is_empty() {
        bail!("操作がありません");
    }
    if !operations
        .iter()
        .any(|operation| matches!(operation.command, Command::Eval(_)))
    {
        bail!("eval がありません");
    }
    Ok(operations)
}

pub(crate) fn eval_groups(operations: &[Operation]) -> Vec<EvalGroup> {
    let mut groups = Vec::new();
    let mut context = String::new();
    let mut input = String::new();
    let mut index = 0;
    while index < operations.len() {
        match &operations[index].command {
            Command::Context(value) => {
                context.clone_from(value);
                input.clear();
                index += 1;
            }
            Command::Input(value) => {
                input.clone_from(value);
                index += 1;
            }
            Command::Eval(_) => {
                let line = operations[index].line;
                let mut outputs = Vec::new();
                while let Some(Operation {
                    command: Command::Eval(output),
                    ..
                }) = operations.get(index)
                {
                    outputs.push(output.clone());
                    index += 1;
                }
                groups.push(EvalGroup {
                    line,
                    context: context.clone(),
                    input: input.clone(),
                    outputs,
                });
            }
        }
    }
    groups
}
