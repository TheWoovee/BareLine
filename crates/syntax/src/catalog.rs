// SPDX-License-Identifier: MPL-2.0
//! Version-one native language metadata. Lexilla names describe bindings, not availability.
use crate::Language;
use std::path::Path;
pub const CATALOG_VERSION: u32 = 1;
#[derive(Clone, Copy, Debug)]
pub struct LanguageMetadata {
    pub language: Language,
    pub id: &'static str,
    pub label: &'static str,
    pub extensions: &'static [&'static str],
    /// Exact file names (case-insensitive) that take precedence over extensions.
    pub filenames: &'static [&'static str],
    /// `#!` interpreter names; a numeric suffix (`python3.12`) also matches.
    pub shebangs: &'static [&'static str],
    pub lexilla: &'static str,
    pub line_comment: Option<&'static str>,
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Newline-separated sets in the lexer's word-list order; the first set
    /// holds the primary keywords. Words may be empty for lexers without lists.
    pub keywords: &'static str,
    pub indent_after: &'static str,
}
impl LanguageMetadata {
    /// Versioned data definition consumed by the same bounded native engine.
    /// Built-in Rust/raw-string handling additionally uses its typed language ID.
    pub fn native_definition(&self) -> crate::udl::Definition {
        crate::udl::Definition {
            version: 1,
            id: self.id.into(),
            name: self.label.into(),
            extensions: self.extensions.iter().map(|s| (*s).into()).collect(),
            keywords: self.keywords.split_ascii_whitespace().map(str::to_owned).collect(),
            operators: "{}[]():;,.+-*/%=!<>|&^?~".into(),
            line_comment: self.line_comment.map(str::to_owned),
            block_comment: self.block_comment.map(|(a, b)| (a.into(), b.into())),
            strings: if self.language == Language::Json {
                vec!['"']
            } else if matches!(
                self.language,
                Language::JavaScript | Language::TypeScript | Language::Go
            ) {
                vec!['"', '\'', '`']
            } else {
                vec!['"', '\'']
            },
            fold_pairs: vec![('{', '}'), ('[', ']')],
        }
    }
}
macro_rules! entry {
    ($lang:ident,$id:literal,$label:literal,$ext:expr,$names:expr,$shebangs:expr,$lexer:literal,$line:expr,$block:expr,$keys:expr,$indent:literal) => {
        LanguageMetadata {
            language: Language::$lang,
            id: $id,
            label: $label,
            extensions: $ext,
            filenames: $names,
            shebangs: $shebangs,
            lexilla: $lexer,
            line_comment: $line,
            block_comment: $block,
            keywords: $keys,
            indent_after: $indent,
        }
    };
}
const C_WORDS: &str = "auto break case char const continue default do double else enum extern float for goto if int long register return short signed sizeof static struct switch typedef union unsigned void volatile while";
const JS_WORDS: &str = "async await break case catch class const continue debugger default delete do else export extends finally for from function if import in instanceof let new of return static super switch this throw try typeof var void while with yield true false null undefined";
const CSS_WORDS: &str =
    "important inherit initial unset none auto flex grid block inline relative absolute fixed sticky";
const FORTRAN_WORDS: &str = "allocatable allocate assign associate backspace block call case character class close common complex contains continue cycle data deallocate default dimension do double else elseif elsewhere end enddo endif entry enum equivalence exit extends external forall format function go goto if implicit in include inout integer intent interface intrinsic kind len logical module namelist none nullify only open operator optional out parameter pause pointer precision print private procedure program protected public pure read real recursive result return rewind save select sequence stop subroutine target then type use value where while write";
pub static CATALOG: &[LanguageMetadata] = &[
    entry!(
        C,
        "c",
        "C",
        &["c", "h"],
        &[],
        &[],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        C_WORDS,
        "{"
    ),
    entry!(
        Cpp,
        "cpp",
        "C++",
        &["cpp", "cxx", "cc", "hpp", "hxx"],
        &[],
        &[],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "class namespace template typename public private protected virtual override nullptr constexpr using new delete true false auto const struct return if else for while",
        "{"
    ),
    entry!(
        CSharp,
        "csharp",
        "C#",
        &["cs"],
        &[],
        &[],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "using namespace class interface public private protected internal static async await var string int bool new return if else foreach true false null",
        "{"
    ),
    entry!(
        Java,
        "java",
        "Java",
        &["java"],
        &[],
        &[],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "package import class interface public private protected static final void int boolean new return if else for while try catch throw throws true false null",
        "{"
    ),
    entry!(
        JavaScript,
        "javascript",
        "JavaScript",
        &["js", "mjs", "cjs", "jsx"],
        &[],
        &["node", "nodejs", "deno", "bun"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        JS_WORDS,
        "{"
    ),
    entry!(
        TypeScript,
        "typescript",
        "TypeScript",
        &["ts", "tsx", "mts", "cts"],
        &[],
        &["ts-node", "tsx"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "async await class const declare enum export extends function implements import interface let namespace new private protected public readonly return static string number boolean type typeof undefined unknown never void true false null",
        "{"
    ),
    entry!(
        Python,
        "python",
        "Python",
        &["py", "pyw"],
        &[],
        &["python", "pypy"],
        "python",
        Some("#"),
        None,
        "False None True and as assert async await break class continue def del elif else except finally for from global if import in is lambda nonlocal not or pass raise return try while with yield",
        ":"
    ),
    entry!(
        Rust,
        "rust",
        "Rust",
        &["rs"],
        &[],
        &[],
        "rust",
        Some("//"),
        Some(("/*", "*/")),
        crate::RUST_KEYWORDS,
        "{"
    ),
    entry!(
        Go,
        "go",
        "Go",
        &["go"],
        &[],
        &[],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "break case chan const continue default defer else fallthrough for func go goto if import interface map package range return select struct switch type var true false nil",
        "{"
    ),
    entry!(
        Html,
        "html",
        "HTML",
        &["html", "htm", "xhtml"],
        &[],
        &[],
        "hypertext",
        None,
        Some(("<!--", "-->")),
        "html head body title script style div span a p input form button table tr td meta link",
        ">"
    ),
    entry!(
        Css,
        "css",
        "CSS",
        &["css"],
        &[],
        &[],
        "css",
        None,
        Some(("/*", "*/")),
        CSS_WORDS,
        "{"
    ),
    entry!(
        Json,
        "json",
        "JSON",
        &["json", "jsonl"],
        &[],
        &[],
        "json",
        None,
        None,
        "true false null",
        "{["
    ),
    entry!(
        Xml,
        "xml",
        "XML",
        &["xml", "svg", "xsd", "xsl", "xslt", "xaml", "resx", "plist"],
        &[],
        &[],
        "xml",
        None,
        Some(("<!--", "-->")),
        "xml version encoding",
        ">"
    ),
    entry!(
        Sql,
        "sql",
        "SQL",
        &["sql"],
        &[],
        &[],
        "sql",
        Some("--"),
        Some(("/*", "*/")),
        "SELECT FROM WHERE INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE DROP ALTER JOIN LEFT RIGHT INNER OUTER ON AS AND OR NOT NULL IS ORDER BY GROUP HAVING LIMIT DISTINCT UNION ALL true false select from where insert into values update set delete create table join on as and or not null",
        "("
    ),
    entry!(
        Toml,
        "toml",
        "TOML",
        &["toml"],
        &["Cargo.lock"],
        &[],
        "toml",
        Some("#"),
        None,
        "true false",
        "[{"
    ),
    // Shells, scripts and build files. Case-insensitive lexers expect lower-case words.
    entry!(
        PowerShell,
        "powershell",
        "PowerShell",
        &["ps1", "psm1", "psd1"],
        &[],
        &["pwsh", "powershell"],
        "powershell",
        Some("#"),
        Some(("<#", "#>")),
        "begin break catch class continue data default define do dynamicparam else elseif end enum exit filter finally for foreach from function hidden if in param process return static switch throw trap try until using var while workflow",
        "{"
    ),
    entry!(
        Batch,
        "batch",
        "Batch",
        &["bat", "cmd"],
        &[],
        &[],
        "batch",
        Some("REM"),
        None,
        "assoc break call cd chdir cls color copy date del dir echo endlocal erase exit for ftype goto if md mkdir mklink move not path pause popd prompt pushd rd rem ren rename rmdir set setlocal shift start time title type ver verify vol exist defined errorlevel else equ neq lss leq gtr geq nul",
        "("
    ),
    entry!(
        Shell,
        "shell",
        "Shell (Bash)",
        &["sh", "bash", "zsh", "ksh"],
        &[
            ".bashrc",
            ".bash_profile",
            ".bash_login",
            ".bash_logout",
            ".profile",
            ".zshrc",
            ".zshenv",
            ".zprofile",
            "PKGBUILD",
        ],
        &["sh", "bash", "zsh", "ksh", "mksh", "dash", "ash"],
        "bash",
        Some("#"),
        None,
        "case do done elif else esac fi for function if in select then time until while break continue return exit export local readonly declare typeset unset shift source alias eval exec set trap true false echo printf read cd test",
        "{("
    ),
    entry!(
        Yaml,
        "yaml",
        "YAML",
        &["yaml", "yml"],
        &[".clang-format", ".clang-tidy"],
        &[],
        "yaml",
        Some("#"),
        None,
        "true false null yes no on off True False TRUE FALSE Null NULL Yes No",
        ":"
    ),
    entry!(
        Markdown,
        "markdown",
        "Markdown",
        &["md", "markdown", "mdown", "mkd", "mkdn"],
        &[],
        &[],
        "markdown",
        None,
        Some(("<!--", "-->")),
        "",
        ""
    ),
    entry!(
        Ini,
        "ini",
        "INI",
        &["ini", "cfg", "inf"],
        &[".editorconfig", ".gitconfig", ".gitmodules"],
        &[],
        "props",
        Some(";"),
        None,
        "",
        ""
    ),
    entry!(
        Properties,
        "properties",
        "Properties",
        &["properties"],
        &[],
        &[],
        "props",
        Some("#"),
        None,
        "",
        ""
    ),
    entry!(
        Php,
        "php",
        "PHP",
        &["php", "phtml", "php3", "php4", "php5", "php7", "phpt"],
        &[],
        &["php"],
        "hypertext",
        Some("//"),
        Some(("/*", "*/")),
        // An empty first set styles every HTML element; PHP words are the fifth set.
        "\n\n\n\nabstract and array as break callable case catch class clone const continue declare default do echo else elseif empty enddeclare endfor endforeach endif endswitch endwhile enum eval exit extends final finally fn for foreach function global goto if implements include include_once instanceof insteadof interface isset list match namespace new or print private protected public readonly require require_once return static switch throw trait try unset use var while xor yield true false null self parent",
        "{"
    ),
    entry!(
        Perl,
        "perl",
        "Perl",
        &["pl", "pm", "pod"],
        &[],
        &["perl"],
        "perl",
        Some("#"),
        None,
        "__DATA__ __END__ BEGIN END and cmp continue do else elsif eq for foreach ge gt if last le local lt my ne next no not or our package redo require return sub unless until use while xor print say die warn defined undef ref shift push pop keys values exists delete chomp split join map grep sort scalar wantarray",
        "{"
    ),
    entry!(
        Ruby,
        "ruby",
        "Ruby",
        &["rb", "rbw", "rake", "gemspec", "ru"],
        &["Rakefile", "Gemfile", "Guardfile", "Vagrantfile", "Podfile", "Brewfile"],
        &["ruby"],
        "ruby",
        Some("#"),
        Some(("=begin", "=end")),
        "__ENCODING__ __FILE__ __LINE__ BEGIN END alias and begin break case class def defined? do else elsif end ensure false for if in module next nil not or redo rescue retry return self super then true undef unless until when while yield require require_relative attr_accessor attr_reader attr_writer include extend private protected public raise lambda proc puts",
        ""
    ),
    entry!(
        Lua,
        "lua",
        "Lua",
        &["lua", "wlua", "rockspec"],
        &[],
        &["lua", "luajit"],
        "lua",
        Some("--"),
        Some(("--[[", "]]")),
        "and break do else elseif end false for function goto if in local nil not or repeat return then true until while\nassert collectgarbage dofile error getmetatable ipairs load loadfile next pairs pcall print rawequal rawget rawlen rawset require select setmetatable tonumber tostring type xpcall",
        ""
    ),
    entry!(
        Makefile,
        "makefile",
        "Makefile",
        &["mk", "mak", "make"],
        &["Makefile", "GNUmakefile", "BSDmakefile"],
        &["make"],
        "makefile",
        Some("#"),
        None,
        "define endef export else endif ifdef ifeq ifndef ifneq include -include sinclude override private undefine unexport vpath",
        ":"
    ),
    entry!(
        Dockerfile,
        "dockerfile",
        "Dockerfile",
        &["dockerfile"],
        &["Dockerfile", "Containerfile"],
        &[],
        // No upstream Dockerfile lexer: shell rules cover RUN lines, comments and strings.
        "bash",
        Some("#"),
        None,
        "ADD ARG CMD COPY ENTRYPOINT ENV EXPOSE FROM HEALTHCHECK LABEL MAINTAINER ONBUILD RUN SHELL STOPSIGNAL USER VOLUME WORKDIR AS",
        "\\"
    ),
    entry!(
        CMake,
        "cmake",
        "CMake",
        &["cmake"],
        &["CMakeLists.txt"],
        &[],
        "cmake",
        Some("#"),
        None,
        "add_compile_definitions add_compile_options add_custom_command add_custom_target add_definitions add_dependencies add_executable add_library add_subdirectory add_test cmake_minimum_required configure_file enable_testing file find_package find_library find_path find_program function endfunction get_filename_component include include_directories install list macro endmacro message option project return set set_property set_target_properties string target_compile_definitions target_compile_features target_compile_options target_include_directories target_link_libraries target_link_options target_sources unset\nAND OR NOT STREQUAL EQUAL LESS GREATER MATCHES DEFINED EXISTS PUBLIC PRIVATE INTERFACE REQUIRED COMPONENTS STATIC SHARED MODULE CACHE PARENT_SCOPE FORCE TARGET TARGETS DESTINATION VERSION LANGUAGES ON OFF TRUE FALSE",
        "("
    ),
    entry!(
        Diff,
        "diff",
        "Diff",
        &["diff", "patch", "rej"],
        &[],
        &[],
        "diff",
        None,
        None,
        "",
        ""
    ),
    entry!(
        Log,
        "log",
        "Log",
        &["log"],
        &[],
        &[],
        // Compiler, tool and traceback lines; see CppMode::ErrorList.
        "errorlist",
        None,
        None,
        "",
        ""
    ),
    entry!(
        VisualBasic,
        "vb",
        "Visual Basic",
        &["vb", "bas", "frm"],
        &[],
        &[],
        "vb",
        Some("'"),
        None,
        "addhandler addressof alias and andalso as boolean byref byte byval call case catch cbool cbyte cchar cdate cdbl cdec char cint class clng cobj const continue csbyte cshort csng cstr ctype cuint culng cushort date decimal declare default delegate dim directcast do double each else elseif end enum erase error event exit false finally for friend function get gettype global gosub goto handles if implements imports in inherits integer interface is isnot let lib like long loop me mod module mustinherit mustoverride mybase myclass namespace narrowing new next not nothing notinheritable notoverridable object of on operator option optional or orelse overloads overridable overrides paramarray partial private property protected public raiseevent readonly redim rem removehandler resume return sbyte select set shadows shared short single static step stop string structure sub synclock then throw to true try trycast typeof uinteger ulong ushort using variant wend when while widening with withevents writeonly xor",
        ""
    ),
    entry!(
        VbScript,
        "vbscript",
        "VBScript",
        &["vbs"],
        &[],
        &[],
        "vbscript",
        Some("'"),
        None,
        "and as byref byval call case class const dim do each else elseif empty end eqv erase error exit explicit false for function get goto if imp in is let loop mod new next not nothing null on option or preserve private property public randomize redim rem resume select set step sub then to true until wend while with xor",
        ""
    ),
    entry!(
        Pascal,
        "pascal",
        "Pascal",
        &["pas", "pp", "dpr", "lpr", "dpk"],
        &[],
        &[],
        "pascal",
        Some("//"),
        Some(("{", "}")),
        "absolute abstract and array as asm assembler begin case class const constructor destructor dispinterface div do downto else end except exports external file finalization finally for forward function goto if implementation in inherited initialization inline interface is label library message mod nil not object of on or out overload override packed private procedure program property protected public published raise record repeat resourcestring set shl shr string then threadvar to try type unit until uses var virtual while with xor",
        ""
    ),
    entry!(
        Fortran,
        "fortran",
        "Fortran",
        &["f90", "f95", "f03", "f08", "f18"],
        &[],
        &[],
        "fortran",
        Some("!"),
        None,
        FORTRAN_WORDS,
        ""
    ),
    entry!(
        Fortran77,
        "fortran77",
        "Fortran (fixed form)",
        &["f", "for", "f77", "ftn"],
        &[],
        &[],
        "f77",
        Some("!"),
        None,
        FORTRAN_WORDS,
        ""
    ),
    entry!(
        Assembly,
        "asm",
        "Assembly",
        &["asm", "s", "nasm"],
        &[],
        &[],
        "asm",
        Some(";"),
        None,
        // CPU instructions, FPU instructions, registers, directives.
        "aaa aad aam aas adc add and call cbw cdq cdqe clc cld cli cmc cmp cmpsb cmpsd cmpsw cpuid cqo cwd daa das dec div enter hlt idiv imul in inc int into iret ja jae jb jbe jc jcxz je jecxz jg jge jl jle jmp jna jnae jnb jnbe jnc jne jng jnge jnl jnle jno jnp jns jnz jo jp jpe jpo js jz lahf lea leave lodsb lodsd lodsw loop loope loopne mov movsb movsd movsw movsx movzx mul neg nop not or out pop popa popf push pusha pushf rcl rcr ret retn rol ror sahf sal sar sbb scasb scasd scasw shl shr stc std sti stosb stosd stosw sub syscall test xchg xlat xor\nfld fst fstp fadd fsub fmul fdiv fcom fcomp fxch finit\nal ah ax eax rax bl bh bx ebx rbx cl ch cx ecx rcx dl dh dx edx rdx si esi rsi di edi rdi sp esp rsp bp ebp rbp r8 r9 r10 r11 r12 r13 r14 r15 cs ds es fs gs ss\n.code .data .model .stack .const align bits byte db dd dq dt dw dword end endm endp ends equ extern global include macro org proc public qword section segment times word",
        ""
    ),
    entry!(
        Latex,
        "latex",
        "LaTeX",
        &["tex", "latex", "ltx", "sty", "dtx"],
        &[],
        &[],
        "latex",
        Some("%"),
        None,
        "",
        ""
    ),
    entry!(
        R,
        "r",
        "R",
        &["r"],
        &[".Rprofile"],
        &["Rscript"],
        "r",
        Some("#"),
        None,
        "if else repeat while function for in next break TRUE FALSE NULL Inf NaN NA NA_integer_ NA_real_ NA_character_ return library require",
        "{"
    ),
    // C-family languages without their own upstream lexer use the C++ lexer.
    entry!(
        Swift,
        "swift",
        "Swift",
        &["swift"],
        &[],
        &["swift"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "actor any as associatedtype async await break case catch class continue default defer deinit do else enum extension fallthrough false fileprivate for func guard if import in init inout internal is let nil open operator private protocol public repeat rethrows return self Self some static struct subscript super switch throw throws true try typealias var where while",
        "{"
    ),
    entry!(
        Kotlin,
        "kotlin",
        "Kotlin",
        &["kt", "kts"],
        &[],
        &["kotlin"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "as break by catch class companion constructor continue data do else enum false final finally for fun get if import in init inline interface internal is lateinit null object open override package private protected public return sealed set super suspend this throw true try typealias typeof val var vararg when while",
        "{"
    ),
    entry!(
        Scala,
        "scala",
        "Scala",
        &["scala", "sc"],
        &[],
        &["scala"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "abstract case catch class def do else enum export extends false final finally for forSome given if implicit import lazy match new null object override package private protected return sealed super then this throw trait true try type val var while with yield",
        "{"
    ),
    entry!(
        Groovy,
        "groovy",
        "Groovy",
        &["groovy", "gradle", "gvy"],
        &["Jenkinsfile"],
        &["groovy"],
        "cpp",
        Some("//"),
        Some(("/*", "*/")),
        "abstract as assert boolean break byte case catch char class continue def default do double else enum extends false final finally float for if implements import in instanceof int interface long new null package private protected public return short static super switch synchronized this throw throws trait true try var void while",
        "{"
    ),
    entry!(
        Dart,
        "dart",
        "Dart",
        &["dart"],
        &[],
        &["dart"],
        "dart",
        Some("//"),
        Some(("/*", "*/")),
        // Primary keywords, then (after two unused sets) built-in types.
        "abstract as assert async await base break case catch class const continue covariant default deferred do dynamic else enum export extends extension external factory false final finally for get hide if implements import in interface is late library mixin new null on operator part required rethrow return sealed set show static super switch sync this throw true try typedef var void when while with yield\n\n\nbool double int num String List Map Set Object Function Future Stream Iterable Never Null",
        "{"
    ),
    entry!(
        Haskell,
        "haskell",
        "Haskell",
        &["hs"],
        &[],
        &["runhaskell", "runghc"],
        "haskell",
        Some("--"),
        Some(("{-", "-}")),
        "case class data default deriving do else family forall foreign hiding if import in infix infixl infixr instance let mdo module newtype of proc qualified rec then type where",
        ""
    ),
    entry!(
        Erlang,
        "erlang",
        "Erlang",
        &["erl", "hrl"],
        &["rebar.config"],
        &["escript"],
        "erlang",
        Some("%"),
        None,
        "after and andalso band begin bnot bor bsl bsr bxor case catch cond div end fun if let maybe not of or orelse receive rem try when xor",
        ""
    ),
    entry!(
        Tcl,
        "tcl",
        "Tcl",
        &["tcl", "tk", "tm"],
        &[],
        &["tclsh", "wish", "expect"],
        "tcl",
        Some("#"),
        None,
        "after append array break catch cd close concat continue dict else elseif eof error eval exec exit expr file for foreach format gets glob global if incr info join lappend lindex linsert list llength lrange lreplace lsearch lset lsort namespace open package proc puts read regexp regsub rename return set source split string switch tell time trace unset update uplevel upvar variable vwait while",
        "{"
    ),
    // Windows installers and configuration.
    entry!(
        AutoIt,
        "autoit",
        "AutoIt",
        &["au3"],
        &[],
        &[],
        "au3",
        Some(";"),
        Some(("#cs", "#ce")),
        "and byref case const continuecase continueloop default dim do else elseif endfunc endif endselect endswitch endwith enum exit exitloop false for func global if in local next not null or redim return select static step switch then to true until volatile wend while with",
        ""
    ),
    entry!(
        Nsis,
        "nsis",
        "NSIS",
        &["nsi", "nsh"],
        &[],
        &[],
        "nsis",
        Some(";"),
        Some(("/*", "*/")),
        "Abort BringToFront Call CallInstDLL ClearErrors CopyFiles CreateDirectory CreateShortCut Delete DeleteRegKey DeleteRegValue DetailPrint Exch Exec ExecShell ExecWait File FileClose FileOpen FileRead FileWrite Function FunctionEnd Goto Icon IfErrors IfFileExists InstallDir IntCmp IntOp LangString MessageBox Name OutFile Page Pop Push ReadRegStr RequestExecutionLevel Return RMDir Section SectionEnd SectionGroup SectionGroupEnd SetOutPath SetShellVarContext ShowInstDetails ShowUnInstDetails StrCmp StrCpy StrLen Unicode Var WriteRegDWORD WriteRegStr WriteUninstaller",
        ""
    ),
    entry!(
        InnoSetup,
        "inno",
        "Inno Setup",
        &["iss", "isl"],
        &[],
        &[],
        "inno",
        Some(";"),
        None,
        // Sections, directives, parameters, preprocessor, Pascal script.
        "code components custommessages dirs files icons ini installdelete langoptions languages messages registry run setup tasks types uninstalldelete uninstallrun\nallownoicons appcopyright appid appname apppublisher apppublisherurl appsupporturl appupdatesurl appversion architecturesallowed architecturesinstallin64bitmode compression defaultdirname defaultgroupname disableprogramgrouppage licensefile outputbasefilename outputdir privilegesrequired setupiconfile solidcompression uninstalldisplayicon wizardstyle\nafterinstall beforeinstall check components description destdir destname filename flags iconfilename languages name parameters root source subkey tasks types valuedata valuename valuetype workingdir\nappend define elif else emit endif endsub error expr for if ifdef ifndef include insert pragma sub undef\nand begin case const do else end except exit false finally for function if nil not of or procedure repeat result then to true try until var while",
        ""
    ),
    entry!(
        Registry,
        "registry",
        "Registry",
        &["reg"],
        &[],
        &[],
        "registry",
        Some(";"),
        None,
        "",
        ""
    ),
    // Style sheet dialects share the CSS lexer; see CppMode::Scss and CppMode::Less.
    entry!(
        Scss,
        "scss",
        "SCSS",
        &["scss", "sass"],
        &[],
        &[],
        "css",
        Some("//"),
        Some(("/*", "*/")),
        CSS_WORDS,
        "{"
    ),
    entry!(
        Less,
        "less",
        "Less",
        &["less"],
        &[],
        &[],
        "css",
        Some("//"),
        Some(("/*", "*/")),
        CSS_WORDS,
        "{"
    ),
    // Further languages with a dedicated upstream lexer.
    entry!(
        Ada,
        "ada",
        "Ada",
        &["adb", "ads", "ada"],
        &[],
        &[],
        "ada",
        Some("--"),
        None,
        "abort abs abstract accept access aliased all and array at begin body case constant declare delay delta digits do else elsif end entry exception exit for function generic goto if in interface is limited loop mod new not null of or others out overriding package pragma private procedure protected raise range record rem renames requeue return reverse select separate some subtype synchronized tagged task terminate then type until use when while with xor",
        ""
    ),
    entry!(
        D,
        "d",
        "D",
        &["d", "di"],
        &[],
        &["rdmd"],
        "d",
        Some("//"),
        Some(("/*", "*/")),
        "abstract alias align asm assert auto body bool break byte case cast catch char class const continue dchar debug default delegate deprecated do double else enum export extern false final finally float for foreach foreach_reverse function goto if immutable import in inout int interface invariant is lazy long mixin module new nothrow null out override package pragma private protected public pure real ref return scope shared short static struct super switch synchronized template this throw true try typeid typeof ubyte uint ulong union unittest ushort version void wchar while with",
        "{"
    ),
    entry!(
        FSharp,
        "fsharp",
        "F#",
        &["fs", "fsi", "fsx"],
        &[],
        &[],
        "fsharp",
        Some("//"),
        Some(("(*", "*)")),
        "abstract and as assert base begin class default delegate do done downcast downto elif else end exception extern false finally fixed for fun function global if in inherit inline interface internal lazy let match member module mutable namespace new not null of open or override private public rec return select sig static struct then to true try type upcast use val void when while with yield",
        ""
    ),
    entry!(
        Julia,
        "julia",
        "Julia",
        &["jl"],
        &[],
        &["julia"],
        "julia",
        Some("#"),
        Some(("#=", "=#")),
        "abstract baremodule begin break catch const continue do else elseif end export false finally for function global if import in isa let local macro module mutable primitive quote return struct true try type using where while",
        ""
    ),
    entry!(
        Lisp,
        "lisp",
        "Lisp",
        &["lisp", "lsp", "el", "asd"],
        &[],
        &["sbcl", "clisp"],
        "lisp",
        Some(";"),
        Some(("#|", "|#")),
        "and apply car case cdr cond cons defclass defconstant defgeneric defmacro defmethod defparameter defstruct defun defvar do dolist dotimes flet funcall function if labels lambda let let* list loop nil not or prog1 progn quote return setf setq t unless when",
        "("
    ),
    entry!(
        Matlab,
        "matlab",
        "MATLAB",
        &["m"],
        &[],
        &[],
        "matlab",
        Some("%"),
        Some(("%{", "%}")),
        "break case catch classdef continue else elseif end for function global if otherwise parfor persistent return spmd switch try while",
        ""
    ),
    entry!(
        Nim,
        "nim",
        "Nim",
        &["nim", "nims", "nimble"],
        &[],
        &["nim"],
        "nim",
        Some("#"),
        Some(("#[", "]#")),
        "addr and as asm bind block break case cast concept const continue converter defer discard distinct div do elif else end enum except export finally for from func if import in include interface is isnot iterator let macro method mixin mod nil not notin object of or out proc ptr raise ref return shl shr static template try tuple type using var when while xor yield",
        ":"
    ),
    entry!(
        OCaml,
        "ocaml",
        "OCaml",
        &["ml", "mli"],
        &[],
        &["ocaml"],
        "caml",
        None,
        Some(("(*", "*)")),
        "and as assert begin class constraint do done downto else end exception external false for fun function functor if in include inherit initializer lazy let match method module mutable new nonrec object of open or private rec sig struct then to true try type val virtual when while with",
        ""
    ),
    entry!(
        Verilog,
        "verilog",
        "Verilog",
        &["v", "vh", "sv", "svh"],
        &[],
        &[],
        "verilog",
        Some("//"),
        Some(("/*", "*/")),
        "always always_comb always_ff always_latch and assign automatic begin bit buf byte case casex casez class default defparam disable else end endcase endclass endfunction endgenerate endinterface endmodule endpackage endprimitive endspecify endtask enum event for force forever fork function generate genvar if initial inout input int integer interface join localparam logic module nand negedge nor not or output package parameter posedge primitive real reg repeat signed specify struct supply0 supply1 task time tri typedef wait while wire xor",
        ""
    ),
    entry!(
        Vhdl,
        "vhdl",
        "VHDL",
        &["vhd", "vhdl"],
        &[],
        &[],
        "vhdl",
        Some("--"),
        Some(("/*", "*/")),
        "abs access after alias all and architecture array assert attribute begin block body buffer bus case component configuration constant disconnect downto else elsif end entity exit file for function generate generic group guarded if impure in inertial inout is label library linkage literal loop map mod nand new next nor not null of on open or others out package port postponed procedure process pure range record register reject rem report return rol ror select severity shared signal sla sll sra srl subtype then to transport type unaffected units until use variable wait when while with xnor xor",
        ""
    ),
    entry!(
        Zig,
        "zig",
        "Zig",
        &["zig", "zon"],
        &[],
        &[],
        "zig",
        Some("//"),
        None,
        "addrspace align allowzero and anyframe anytype asm async await break callconv catch comptime const continue defer else enum errdefer error export extern false fn for if inline linksection noalias noinline nosuspend null opaque or orelse packed pub resume return struct suspend switch test threadlocal true try undefined union unreachable usingnamespace var volatile while",
        "{"
    ),
    entry!(
        CoffeeScript,
        "coffeescript",
        "CoffeeScript",
        &["coffee", "cson"],
        &["Cakefile"],
        &["coffee"],
        "coffeescript",
        Some("#"),
        None,
        "and break by catch class continue delete do else extends false finally for if in instanceof is isnt loop new no not null of off on or return super switch then this throw true try typeof undefined unless until when while yes yield",
        ""
    ),
    entry!(
        GdScript,
        "gdscript",
        "GDScript",
        &["gd"],
        &[],
        &[],
        "gdscript",
        Some("#"),
        None,
        "and as assert await break breakpoint class class_name const continue elif else enum extends for func if in is match not or pass preload return self signal static super var void while true false null PI TAU INF NAN",
        ":"
    ),
];
static PLAIN: LanguageMetadata = LanguageMetadata {
    language: Language::PlainText,
    id: "text",
    label: "Plain text",
    extensions: &[],
    filenames: &[],
    shebangs: &[],
    lexilla: "null",
    line_comment: None,
    block_comment: None,
    keywords: "",
    indent_after: "",
};
/// The interpreter a `#!` line runs: its last path component, or the first
/// command after `env` and its options or assignments.
fn shebang_program(line: &str) -> Option<&str> {
    let mut words = line.strip_prefix("#!")?.split_ascii_whitespace();
    let program = words.next()?.rsplit(['/', '\\']).next()?;
    let program = if program == "env" {
        words.find(|word| !word.starts_with('-') && !word.contains('='))?
    } else {
        program
    };
    program.rsplit(['/', '\\']).next()
}
fn runs(program: &str, interpreter: &str) -> bool {
    program
        .strip_prefix(interpreter)
        .is_some_and(|version| version.bytes().all(|b| b.is_ascii_digit() || b == b'.'))
}
impl Language {
    pub fn metadata(self) -> &'static LanguageMetadata {
        CATALOG.iter().find(|m| m.language == self).unwrap_or(&PLAIN)
    }
    pub fn label(self) -> &'static str {
        self.metadata().label
    }
    pub fn from_id(id: &str) -> Option<Self> {
        if id == "text" {
            return Some(Self::PlainText);
        }
        CATALOG.iter().find(|m| m.id == id).map(|m| m.language)
    }
    pub fn detect_with_context(path: &Path, prefix: &str, explicit: Option<Self>, association: Option<Self>) -> Self {
        Self::detect_with_regions(path, prefix, "", explicit, association)
    }
    /// Inputs are bounded before parsing; callers read prefix/suffix on a worker.
    pub fn detect_with_regions(
        path: &Path,
        prefix: &str,
        suffix: &str,
        explicit: Option<Self>,
        association: Option<Self>,
    ) -> Self {
        if let Some(language) = explicit.or(association) {
            return language;
        }
        let detected = Self::detect(path);
        if detected != Self::PlainText {
            return detected;
        }
        fn bounded(text: &str) -> &str {
            let mut end = text.len().min(8192);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            &text[..end]
        }
        let prefix = bounded(prefix);
        let suffix = bounded(suffix);
        let first = prefix.lines().next().unwrap_or("");
        if let Some(program) = shebang_program(first)
            && let Some(entry) = CATALOG
                .iter()
                .find(|entry| entry.shebangs.iter().any(|interpreter| runs(program, interpreter)))
        {
            return entry.language;
        }
        for line in prefix.lines().take(5).chain(suffix.lines().rev().take(5)) {
            for marker in ["mode:", "filetype=", "ft="] {
                if let Some((_, value)) = line.split_once(marker) {
                    let id = value
                        .trim_start()
                        .split(|c: char| !c.is_ascii_alphanumeric() && !matches!(c, '+' | '#' | '-'))
                        .next()
                        .unwrap_or("");
                    let alias = match id {
                        "c++" => "cpp",
                        "c#" => "csharp",
                        "js" => "javascript",
                        "ts" => "typescript",
                        "py" => "python",
                        "sh" | "bash" | "zsh" | "ksh" | "shell-script" => "shell",
                        "ps1" | "pwsh" => "powershell",
                        "dosbatch" | "bat" | "cmd" => "batch",
                        "dosini" | "conf-windows" => "ini",
                        "jproperties" => "properties",
                        "yml" => "yaml",
                        "md" | "gfm" => "markdown",
                        "make" | "makefile-gmake" => "makefile",
                        "rb" => "ruby",
                        "pl" | "cperl" => "perl",
                        "tex" | "plaintex" => "latex",
                        "nasm" | "masm" => "asm",
                        "patch" => "diff",
                        "iss" => "inno",
                        "systemverilog" => "verilog",
                        "emacs-lisp" => "lisp",
                        "nxml" => "xml",
                        other => other,
                    };
                    if let Some(language) = Self::from_id(alias) {
                        return language;
                    }
                }
            }
        }
        let trimmed = prefix.trim_start();
        if trimmed.starts_with("<?xml") {
            Self::Xml
        } else if trimmed.to_ascii_lowercase().starts_with("<!doctype html") {
            Self::Html
        } else if (trimmed.starts_with('{') && trimmed.contains("\":"))
            || (trimmed.starts_with('[') && trimmed.contains("{\""))
        {
            Self::Json
        } else {
            Self::PlainText
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_entry_detects_by_its_extensions_and_file_names() {
        // BIZ-03: 15 original entries plus the bundled-lexer catalog.
        assert_eq!(CATALOG.len(), 66);
        let mut ids = std::collections::BTreeSet::new();
        let mut labels = std::collections::BTreeSet::new();
        let mut extensions = std::collections::BTreeSet::new();
        for entry in CATALOG {
            assert!(ids.insert(entry.id), "duplicate id {}", entry.id);
            assert!(labels.insert(entry.label), "duplicate label {}", entry.label);
            assert!(!entry.lexilla.is_empty());
            assert!(
                !entry.extensions.is_empty() || !entry.filenames.is_empty(),
                "{}",
                entry.id
            );
            assert_eq!(Language::from_id(entry.id), Some(entry.language));
            for extension in entry.extensions {
                assert!(extensions.insert(extension.to_ascii_lowercase()), "shared .{extension}");
                assert_eq!(
                    Language::detect(Path::new(&format!("dir/a.{extension}"))),
                    entry.language,
                    ".{extension}"
                );
                assert_eq!(
                    Language::detect(Path::new(&format!("A.{}", extension.to_ascii_uppercase()))),
                    entry.language
                );
            }
            for name in entry.filenames {
                assert_eq!(Language::detect(&Path::new("dir").join(name)), entry.language, "{name}");
            }
        }
        assert_eq!(
            Language::detect_with_context(Path::new("a.rs"), "", Some(Language::Json), None),
            Language::Json
        );
    }
    #[test]
    fn file_name_and_extension_detection_table() {
        for (path, language) in [
            ("script.ps1", Language::PowerShell),
            ("Module.psm1", Language::PowerShell),
            ("build.BAT", Language::Batch),
            ("setup.cmd", Language::Batch),
            ("install.sh", Language::Shell),
            (".bashrc", Language::Shell),
            ("home/.zshrc", Language::Shell),
            ("docker-compose.yml", Language::Yaml),
            ("README.md", Language::Markdown),
            ("desktop.ini", Language::Ini),
            (".editorconfig", Language::Ini),
            ("app.properties", Language::Properties),
            ("index.php", Language::Php),
            ("lib.pm", Language::Perl),
            ("Gemfile", Language::Ruby),
            ("tasks.rake", Language::Ruby),
            ("init.lua", Language::Lua),
            ("Makefile", Language::Makefile),
            ("makefile", Language::Makefile),
            ("rules.mk", Language::Makefile),
            ("Dockerfile", Language::Dockerfile),
            ("web.dockerfile", Language::Dockerfile),
            ("CMakeLists.txt", Language::CMake),
            ("notes.txt", Language::PlainText),
            ("toolchain.cmake", Language::CMake),
            ("fix.patch", Language::Diff),
            ("changes.diff", Language::Diff),
            ("server.log", Language::Log),
            ("Form1.vb", Language::VisualBasic),
            ("logon.vbs", Language::VbScript),
            ("unit1.pas", Language::Pascal),
            ("solver.f90", Language::Fortran),
            ("legacy.f", Language::Fortran77),
            ("boot.asm", Language::Assembly),
            ("paper.tex", Language::Latex),
            ("analysis.R", Language::R),
            ("App.swift", Language::Swift),
            ("build.gradle.kts", Language::Kotlin),
            ("Main.scala", Language::Scala),
            ("Jenkinsfile", Language::Groovy),
            ("main.dart", Language::Dart),
            ("Main.hs", Language::Haskell),
            ("server.erl", Language::Erlang),
            ("app.tcl", Language::Tcl),
            ("macro.au3", Language::AutoIt),
            ("installer.nsi", Language::Nsis),
            ("setup.iss", Language::InnoSetup),
            ("tweak.reg", Language::Registry),
            ("site.scss", Language::Scss),
            ("theme.less", Language::Less),
            ("Cargo.lock", Language::Toml),
            ("src/main.rs", Language::Rust),
            ("no_extension", Language::PlainText),
        ] {
            assert_eq!(Language::detect(Path::new(path)), language, "{path}");
        }
    }
    #[test]
    fn shebang_detection_table() {
        for (line, language) in [
            ("#!/usr/bin/env python3", Language::Python),
            ("#!/usr/bin/python3.12 -u", Language::Python),
            ("#!/usr/bin/env node", Language::JavaScript),
            ("#!/usr/bin/env -S deno run --allow-read", Language::JavaScript),
            ("#!/bin/sh", Language::Shell),
            ("#!/bin/bash -e", Language::Shell),
            ("#!/usr/bin/env zsh", Language::Shell),
            ("#!/usr/bin/env pwsh", Language::PowerShell),
            ("#!/usr/bin/perl -w", Language::Perl),
            ("#!/usr/bin/env ruby", Language::Ruby),
            ("#!/usr/bin/env lua5.4", Language::Lua),
            ("#!/usr/bin/env php", Language::Php),
            ("#!/usr/bin/make -f", Language::Makefile),
            ("#!/usr/bin/env Rscript", Language::R),
            ("#!/usr/bin/tclsh", Language::Tcl),
            ("#!/usr/bin/env escript", Language::Erlang),
            ("#!/usr/bin/env runghc", Language::Haskell),
            ("#!/usr/bin/env swift", Language::Swift),
            ("#!/usr/bin/env LANG=C julia", Language::Julia),
            ("#!/usr/bin/env fish", Language::PlainText),
            ("#!/usr/bin/env shellcheck", Language::PlainText),
            ("#!", Language::PlainText),
            ("# not a shebang: python", Language::PlainText),
        ] {
            let text = format!("{line}\r\nbody\n");
            assert_eq!(
                Language::detect_with_context(Path::new("tool"), &text, None, None),
                language,
                "{line}"
            );
        }
        // A known extension wins over the interpreter line.
        assert_eq!(
            Language::detect_with_context(Path::new("tool.rb"), "#!/bin/sh\n", None, None),
            Language::Ruby
        );
    }
    #[test]
    fn modelines_accept_editor_aliases() {
        for (line, language) in [
            ("# vim: set ft=sh:", Language::Shell),
            ("# -*- mode: shell-script -*-", Language::Shell),
            ("rem vim: ft=dosbatch", Language::Batch),
            ("# vim: filetype=yaml", Language::Yaml),
            ("% -*- mode: latex -*-", Language::Latex),
            ("; vim: ft=dosini", Language::Ini),
        ] {
            assert_eq!(
                Language::detect_with_context(Path::new("tool"), line, None, None),
                language,
                "{line}"
            );
        }
    }
}

/// Bounded filename glob associations. Explicit names take precedence over
/// wildcard patterns; malformed/oversized data never broadens a match.
pub fn association(path: &Path, associations: &std::collections::BTreeMap<String, String>) -> Option<Language> {
    let name = path.file_name()?.to_str()?;
    if name.len() > 1024 {
        return None;
    }
    if let Some(language) = associations.iter().take(512).find_map(|(pattern, id)| {
        pattern
            .eq_ignore_ascii_case(name)
            .then(|| Language::from_id(id))
            .flatten()
    }) {
        return Some(language);
    }
    associations.iter().take(512).find_map(|(pattern, id)| {
        if pattern.len() > 256 || pattern.contains(['/', '\\']) {
            return None;
        }
        glob(pattern.as_bytes(), name.as_bytes())
            .then(|| Language::from_id(id))
            .flatten()
    })
}
fn glob(pattern: &[u8], name: &[u8]) -> bool {
    let (mut p, mut n, mut star, mut retry) = (0, 0, None, 0);
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&name[n])) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            p += 1;
            retry = n;
        } else if let Some(at) = star {
            retry += 1;
            n = retry;
            p = at + 1;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod association_tests {
    use super::*;
    #[test]
    fn bounded_globs_and_exact_names_have_deterministic_precedence() {
        let mut map = std::collections::BTreeMap::new();
        map.insert("*.test.??".into(), "rust".into());
        map.insert("*.txt".into(), "python".into());
        map.insert("special.txt".into(), "json".into());
        assert_eq!(association(Path::new("thing.test.RS"), &map), Some(Language::Rust));
        assert_eq!(association(Path::new("SPECIAL.TXT"), &map), Some(Language::Json));
        assert_eq!(association(Path::new("other.txt"), &map), Some(Language::Python));
        assert_eq!(association(Path::new("unmatched"), &map), None);
    }
}
