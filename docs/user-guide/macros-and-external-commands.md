# Macros and external commands

## Macros

Commands are in the **Macro** menu.

| Task | Command |
| --- | --- |
| Record | **Start Macro Recording**, then **Stop Macro Recording** and give the macro a name. |
| Play | **Play Selected Macro**, **Play Macro N Times**, or **Play Macro Until End of File**. **Cancel Macro Playback** stops it. |
| Manage | **Manage Macros…** lists saved macros; **Rename Selected Macro**, **Assign Macro Shortcut**, **Set Macro Typing Delay**. |
| Keep and share | **Save Macros** writes them to the `macros` folder of your profile. **Import Macro…** and **Export Selected Macro…** read and write single macros as TOML files. |

What a macro contains: the text you type and paste, keyboard caret movement and selection, deletion, Undo and Redo, Select All, Find Next and Find Previous together with the search they used (text, mode and options), and editor commands such as line, case, indentation and comment commands. Mouse clicks that place the caret are not recorded. A macro stores command IDs and explicit text, never key presses, so it replays the same way whichever shortcut preset is active.

Recording is all or nothing: if you use an action that a macro could not replay exactly, recording stops and is discarded with a message, so a saved macro never differs from what you did.

Playback:

- A whole playback is undone with one **Undo**, in the normal editor and in large-file mode.
- If a step fails, playback stops at that step and says why; **Resume Failed Macro** continues from there.
- A macro has at most 10,000 steps and 4 MiB. An imported macro is checked against the commands a macro can replay before it is accepted.
- A saved macro that cannot be read is set aside and left on disk; the other macros stay usable. **Retry Loading Macros** reads them again.

Notepad++ macros (`shortcuts.xml`) are not imported.

## Run (F5)

**Run > Run…** (**F5**) runs one program with arguments, like Notepad++'s Run dialog. Type a command line such as:

```text
python "$(FULL_CURRENT_PATH)"
C:\tools\lint.exe --file "$(FULL_CURRENT_PATH)" --line $(CURRENT_LINE)
```

- The program is an absolute path or a name found on `PATH`. The current folder is never searched. The name is looked up in the background; **Cancel** stops waiting when a network folder on `PATH` is slow.
- Before anything runs, Bareline shows the resolved program and its arguments and asks you to confirm. Nothing runs without that confirmation.
- The program starts in the active document's folder; `cmd.exe` and batch files start in the workspace folder, so that a bare tool name never resolves against the document's folder. When that folder is not available, Bareline uses the workspace folder, else the active document's folder, else your user profile folder, and never the Windows `System32` folder.
- `cmd.exe` lines and batch files run through the system `cmd.exe` from the Windows `System32` folder. Every other program is started directly, without a shell.
- Output is captured in the **Output** panel (**Run > Output**: **Clear Output**, **Copy Output**, **Save Output…**). The panel keeps the last 1 MiB of output. **Cancel External Command** ends the program and every process it started.

### Variables

| Notepad++ variable | Bareline placeholder | Value |
| --- | --- | --- |
| `$(FULL_CURRENT_PATH)` | `${file}` | Full path of the active document. |
| `$(CURRENT_DIRECTORY)` | `${dir}` | Its folder. |
| `$(FILE_NAME)` | `${file_name}` | Its file name. |
| `$(NAME_PART)` | `${name_part}` | File name without extension. |
| `$(EXT_PART)` | `${ext_part}` | Extension. |
| `$(CURRENT_WORD)` | `${word}` | The selection, or the word at the caret. Not available in large-file mode. |
| `$(CURRENT_LINE)` | `${line}` | Caret line. |
| `$(CURRENT_COLUMN)` | `${column}` | Caret column. |
| `$(NPP_DIRECTORY)` | `${app_dir}` | Bareline's application folder. |
| | `${selection}` | The selected text. |
| | `${workspace}` | The open workspace folder. |

Other `$(...)` text, such as a PowerShell subexpression, is passed on unchanged. Bareline's `${...}` placeholders always expand, so a Run line cannot contain literal `${...}` text such as PowerShell's `${env:PATH}`; an unknown placeholder is refused.

### Security model

Variables carry text from your documents into another program's command line. A document could contain text crafted to run commands, so Bareline quotes each value for the program that receives it and refuses the run when a value cannot be passed as plain data:

- For `cmd.exe` and batch files, text containing `%`, `!` or `"` is refused. A batch file receives each value as one quoted argument; what the script then does with it is up to the script.
- For PowerShell, text containing `"` is refused, as is a value placed directly after `$`, inside a `#` comment or a here-string, or a value that would set PowerShell's own startup options.
- Any variable is refused for `wscript`, `cscript`, `mshta`, `rundll32`, `wsl`, `bash`, `conhost` and `forfiles`, which can turn arguments into code.
- Any variable is refused in a `cmd.exe` line that also starts PowerShell, another `cmd.exe` or one of those programs, and in a PowerShell command that also starts `cmd.exe`, a batch file, another PowerShell or one of those programs, or that uses `--%`, `Invoke-Expression`, a script block built from text, or a program named by a variable. Run that program directly instead.
- A placeholder may not name the program itself.
- Programs are recognized by their file name; a renamed copy is not. Interpreters such as `python -c` or `node -e` are not recognized: a value passed as a separate argument stays data, but do not put a variable inside their code argument.

The rules are implemented and tested in [`crates/macros/src/process.rs`](../../crates/macros/src/process.rs).

## External command definitions

**Run > Load External Command Definition…** loads a reusable command from a TOML file; **Run Loaded External Command** runs it with the same confirmation, quoting and output capture as Run:

```toml
format_version = 1
name = "Lint current file"
program = 'C:\tools\lint.exe'      # absolute path, required
args = ["--file", "${file}", "--line", "${line}"]
mode = "direct"                    # or "shell" for a cmd.exe command line
capture = true                     # show output in the Output panel
```

A definition file cannot grant permission to run: any field other than these is refused, and workspace settings cannot authorize a process. Each run asks for your confirmation.
