// SPDX-License-Identifier: MPL-2.0
//! Compare commands and paint on the normal split editor surfaces.
use super::*;
use bareline_app::compare::{CompareController, CompareInput, CompareOrigin, CompareSource, CompareState};
use bareline_commands::{CommandId, CommandPresentation, CommandRegistry, CommandSpec};
use bareline_diff::{DiffKind, Direction, MergePolicy, Whitespace};
use bareline_document::DocumentSnapshot;
use bareline_renderer::{DrawOp, LayoutError, Rect, TextBackend};
use bareline_ui::{TAB_HEIGHT, rect, text};
const COLOR_CONTROLS: [(&str, &str); 15] = [
    ("compare.colorAdded", "diff.added"),
    ("compare.accentAdded", "diff.added.overview"),
    ("compare.gutterAdded", "diff.added.gutter"),
    ("compare.colorRemoved", "diff.removed"),
    ("compare.accentRemoved", "diff.removed.overview"),
    ("compare.gutterRemoved", "diff.removed.gutter"),
    ("compare.colorChanged", "diff.changed"),
    ("compare.accentChanged", "diff.changed.overview"),
    ("compare.gutterChanged", "diff.changed.gutter"),
    ("compare.colorMoved", "diff.moved"),
    ("compare.accentMoved", "diff.moved.overview"),
    ("compare.gutterMoved", "diff.moved.gutter"),
    ("compare.colorCurrent", "diff.current"),
    ("compare.accentCurrent", "diff.current.overview"),
    ("compare.gutterCurrent", "diff.current.gutter"),
];
fn compare_input(editor:&bareline_app::workspace::WorkspaceEditor)->CompareInput {
    match editor {bareline_app::workspace::WorkspaceEditor::Resident(e)=>CompareInput::Resident(e.snapshot().clone()),bareline_app::workspace::WorkspaceEditor::Paged(e)=>if e.surface.user_read_only{CompareInput::CapturedPaged(e.snapshot().clone(),e.read_handle())}else{CompareInput::Paged(e.read_handle())}}
}

pub(super) fn register(registry: &mut CommandRegistry) {
    bareline_app::compare::register_commands(registry);
    for (id, title) in [
        ("compare.leftSource", "Change left source"),
        ("compare.rightSource", "Change right source"),
        ("compare.whitespace", "Compare whitespace mode"),
        ("compare.ignoreCase", "Ignore case"),
        ("compare.ignoreBlank", "Ignore blank lines"),
        ("compare.ignoreEol", "Ignore EOL style"),
        ("compare.ignoreBom", "Ignore encoding BOM"),
        ("compare.normalizeTabs", "Normalize tabs"),
        ("compare.syncHorizontal", "Synchronize horizontal scrolling"),
        ("compare.disk", "Compare with disk file…"),
        ("compare.lastSaved", "Compare with last saved version"),
        ("compare.external", "Compare with current disk version"),
    ]
    .into_iter()
    .chain(COLOR_CONTROLS)
    .chain([
        ("compare.themeLight", "Light compare theme"),
        ("compare.themeDark", "Dark compare theme"),
        ("compare.themeSystem", "System compare theme"),
        ("compare.defaults", "Use compare theme defaults"),
        ("compare.colorblind", "Color-blind compare palette"),
        ("compare.applyColor", "Apply compare color"),
        ("compare.generalTab", "General compare options"),
        ("compare.colorsTab", "Compare colors"),
        ("compare.trimEdges", "Ignore leading/trailing whitespace"),
        ("compare.ignoreWhitespace", "Ignore all whitespace"),
        ("compare.resetAdded", "Reset added colors"),
        ("compare.resetRemoved", "Reset removed colors"),
        ("compare.resetChanged", "Reset changed colors"),
        ("compare.resetMoved", "Reset moved colors"),
        ("compare.resetCurrent", "Reset current difference colors"),
    ]) {
        let id = CommandId(id);
        let _ = registry.register(CommandSpec {
            id,
            title,
            category: "Compare",
            shortcut: "",
            action: Action::Contributed(id),
        });
    }
    let commands: Vec<_> = registry
        .entries()
        .filter(|c| c.id.0.starts_with("compare."))
        .map(|c| (c.id, c.title))
        .collect();
    for (id, title) in commands {
        let _ = registry.set_presentation(
            id,
            CommandPresentation {
                menu_path: format!("Tools > Compare > {title}"),
                accessible_name: Some(title.into()),
                ..Default::default()
            },
        );
    }
}

#[derive(Default)]
pub(super) struct CompareRuntime {
    controller: Option<CompareController>,
    documents: Option<[CompareInput; 2]>,
    options_open: bool,
    hits: Vec<(Rect, &'static str)>,
    focus: usize,
    colors_tab: bool,
    active_color: Option<&'static str>,
    color_field: bareline_ui::text_field::TextField,
    color_bounds: Option<Rect>,
    color_focus: bool,
    color_blind: bool,
    saved: Vec<(DocumentSnapshot, String)>,
    saved_paged: Vec<(bareline_editor_surface::paged_view::PagedReadHandle,String)>,
    pending_source: Option<PendingSource>,
    overview: [Option<Rect>;2],
    pending_merge: Option<PendingMerge>,
}
struct PendingMerge {source:CompareInput,cancel:bareline_diff::CancelToken,result:std::sync::mpsc::Receiver<Result<bareline_document::EditTransaction,bareline_diff::ApplyError>>}
impl Drop for PendingMerge{fn drop(&mut self){self.cancel.cancel();}}
struct PendingSource {
    left: CompareInput,
    path: PathBuf,
    previous: Vec<CompareInput>,
    origin: CompareOrigin,
}
impl CompareRuntime {
    pub(super) fn capture(
        &self,
        workspace: &Workspace,
        document_ids: &[(usize, u64)],
    ) -> Option<bareline_file_io::session::SessionCompare> {
        let indices = self.indices(workspace)?;
        let id = |index| {
            document_ids
                .iter()
                .find(|(i, _)| *i == index)
                .map(|(_, id)| *id)
        };
        Some(bareline_file_io::session::SessionCompare {
            version: 1,
            left_document: id(indices[0])?,
            right_document: id(indices[1])?,
            options_json: self.controller.as_ref()?.session().to_json().ok()?,
        })
    }
    pub(super) fn restore(
        &mut self,
        workspace: &mut Workspace,
        views: &mut views::ViewsRuntime,
        state: &bareline_file_io::session::SessionCompare,
        document_indices: &[(u64, usize)],
        notify: std::sync::Arc<dyn Fn() + Send + Sync>,
    ) -> Result<usize, String> {
        let index = |id| {
            document_indices
                .iter()
                .find(|(document, _)| *document == id)
                .map(|(_, index)| *index)
                .ok_or("Compare source was not restored")
        };
        let pair = [index(state.left_document)?, index(state.right_document)?];
        let saved = bareline_app::compare::CompareSession::from_json(&state.options_json)
            .map_err(|e| format!("Invalid compare session: {e:?}"))?;
        let mut controller = CompareController::restore(saved)
            .map_err(|e| format!("Invalid compare session: {e:?}"))?;
        if !views.compare_pair(workspace, pair[0], pair[1]) {
            return Err("Compare sources are not ready".into());
        }
        let snapshots = pair.map(|i| compare_input(&workspace.editors[i]));
        controller
            .start_inputs(snapshots[0].clone(), snapshots[1].clone(), notify)
            .map_err(|e| format!("Compare restore: {e:?}"))?;
        self.documents = Some(snapshots);
        self.controller = Some(controller);
        Ok(pair[0])
    }
    pub(super) fn annotate_context(
        &self,
        context: &mut bareline_commands::CommandContext,
        workspace: Option<&Workspace>,
    ) {
        use bareline_commands::CommandState;
        if workspace.is_none_or(|w| {
            w.editors
                .iter()
                .filter(|e| e.paged() || e.snapshot().is_complete())
                .count()
                < 2
        }) {
            context.states.insert(
                CommandId("compare.open"),
                CommandState::disabled("Open two complete text documents first"),
            );
        }
        let indices = workspace.and_then(|w| self.indices(w));
        for id in [
            "compare.next",
            "compare.previous",
            "compare.swap",
            "compare.recompare",
            "compare.cancel",
            "compare.copyLeftToRight",
            "compare.copyRightToLeft",
            "compare.options",
            "compare.pauseAutomatic",
            "compare.close",
        ] {
            let reason = if indices.is_none() {
                Some("Open a comparison first")
            } else if id == "compare.cancel"
                && self
                    .controller
                    .as_ref()
                    .is_none_or(|c| c.state != CompareState::Running)
            {
                Some("No comparison is running")
            } else {
                None
            };
            if let Some(reason) = reason {
                context
                    .states
                    .insert(CommandId(id), CommandState::disabled(reason));
            }
        }
        if let (Some(workspace), Some(indices), Some(controller)) =
            (workspace, indices, &self.controller)
        {
            let hunk = controller.current_hunk();
            let fresh = hunk.is_some_and(|h| {
                h.left_revision == compare_input(&workspace.editors[indices[0]]).revision()
                    && h.right_revision == compare_input(&workspace.editors[indices[1]]).revision()
            });
            if !fresh {
                for id in [
                    "compare.next",
                    "compare.previous",
                    "compare.copyLeftToRight",
                    "compare.copyRightToLeft",
                ] {
                    context.states.insert(
                        CommandId(id),
                        CommandState::disabled("Recompare to obtain a current difference"),
                    );
                }
            }
            for (id, side) in [
                ("compare.copyLeftToRight", 1),
                ("compare.copyRightToLeft", 0),
            ] {
                if workspace.editors[indices[side]].busy()
                    || workspace.editors[indices[side]].read_only()
                {
                    context.states.insert(
                        CommandId(id),
                        CommandState::disabled("Destination is busy or read-only"),
                    );
                }
            }
        }
    }
    fn indices(&self, workspace: &Workspace) -> Option<[usize; 2]> {
        let documents = self.documents.as_ref()?;
        Some([
            workspace
                .editors
                .iter()
                .position(|e| compare_input(e).same_document(&documents[0]))?,
            workspace
                .editors
                .iter()
                .position(|e| compare_input(e).same_document(&documents[1]))?,
        ])
    }
}

impl Shell {
    /// Command identity plus visible occurrence keeps duplicate actions distinct.
    /// Compare chrome has at most two occurrences of any command; reserve sixteen.
    fn compare_accessibility_hit_id(&self,hit:usize)->Option<u64> {
        let (_,id)=self.compare.hits.get(hit)?;
        let command=self.app.commands.entries().filter(|command|command.id.0.starts_with("compare.")).position(|command|command.id.0==*id)?;
        let occurrence=self.compare.hits[..hit].iter().filter(|(_,candidate)|candidate==id).count();
        Some(50_000+command as u64*16+occurrence as u64)
    }
    pub(super) fn compare_accessibility_nodes(&self)->Vec<bareline_platform::accessibility::AccessibilityNode> {
        use bareline_platform::accessibility::{AccessibilityNode,AccessibilityRole};
        let offset=self.editor_bounds();
        let mut context=bareline_commands::CommandContext::default();
        self.compare.annotate_context(&mut context,self.workspace.as_ref());
        let mut nodes:Vec<_>=self.compare.hits.iter().enumerate().filter_map(|(hit,(bounds,id))| {
            let command=self.app.commands.entries().find(|command|command.id.0==*id)?;
            let mut role=AccessibilityRole::Button;let mut selected=false;let mut value=None;
            if let Some(controller)=&self.compare.controller {
                let options=controller.options();
                let checked=match *id {
                    "compare.trimEdges"=>Some(options.whitespace==Whitespace::TrimEdges),
                    "compare.ignoreWhitespace"=>Some(options.whitespace==Whitespace::IgnoreAll),
                    "compare.ignoreBlank"=>Some(options.ignore_blank_lines),
                    "compare.ignoreCase"=>Some(options.ignore_case),
                    "compare.ignoreEol"=>Some(options.ignore_eol_style),
                    "compare.ignoreBom"=>Some(options.ignore_encoding_bom),
                    "compare.normalizeTabs"=>Some(options.normalize_tabs),
                    "compare.pauseAutomatic"=>Some(controller.pause_automatic),
                    "compare.syncHorizontal"=>Some(controller.sync_horizontal),
                    "compare.colorblind"=>Some(self.compare.color_blind),_=>None,
                };
                if let Some(checked)=checked{role=AccessibilityRole::Checkbox;selected=checked;value=Some(if checked{"On"}else{"Off"}.into());}
                match *id {
                    "compare.whitespace"=>{role=AccessibilityRole::Combo;value=Some(match options.whitespace{Whitespace::Significant=>"Significant",Whitespace::TrimEdges=>"Trim edges",Whitespace::IgnoreAll=>"Ignore all"}.into());},
                    "compare.leftSource"=>value=Some(controller.sources[0].label.clone()),
                    "compare.rightSource"=>value=Some(controller.sources[1].label.clone()),
                    "compare.next" if self.compare.hits[..hit].iter().any(|(_,candidate)|*candidate=="compare.next")=>{let (current,total)=controller.counter();value=Some(format!("Difference {current} of {total}"));},_=>{},
                }
            }
            match *id {
                "compare.generalTab"|"compare.colorsTab"=>{role=AccessibilityRole::Tab;selected=(*id=="compare.colorsTab")==self.compare.colors_tab;},
                "compare.themeLight"|"compare.themeDark"|"compare.themeSystem"=>{role=AccessibilityRole::Radio;selected=matches!((*id,self.settings.effective().theme),("compare.themeLight",bareline_settings::ThemeMode::Light)|("compare.themeDark",bareline_settings::ThemeMode::Dark)|("compare.themeSystem",bareline_settings::ThemeMode::System));value=Some(if selected{"Selected"}else{"Not selected"}.into());},_=>{},
            }
            if let Some((_,key))=COLOR_CONTROLS.iter().find(|(candidate,_)|candidate==id){value=self.settings.theme_color(key).map(|color|format!("#{:06X}",color.0));}
            Some(AccessibilityNode{id:self.compare_accessibility_hit_id(hit)?,parent:1,role,name:command.title.into(),value,
                bounds:[(bounds.x+offset.x) as f64,(bounds.y+offset.y) as f64,bounds.width as f64,bounds.height as f64],
                disabled:context.states.get(&command.id).is_some_and(|state|!state.enabled),selected,expanded:None,focusable:true,invokable:true})
        }).collect();
        if self.compare.active_color.is_some(){if let Some(bounds)=self.compare.color_bounds{nodes.push(AccessibilityNode{id:59_999,parent:1,role:AccessibilityRole::TextField,name:"Compare color hexadecimal value".into(),value:Some(self.compare.color_field.value().into()),bounds:[(bounds.x+offset.x) as f64,(bounds.y+offset.y) as f64,bounds.width as f64,bounds.height as f64],disabled:false,selected:false,expanded:None,focusable:true,invokable:false});}}nodes
    }
    pub(super) fn compare_accessibility_focus(&self)->Option<u64> {
        if !self.compare.options_open{return None;}
        if self.compare.active_color.is_some()&&self.compare.color_focus{return Some(59_999);}
        self.compare_accessibility_hit_id(self.compare.focus)
    }
    pub(super) fn compare_accessibility(&mut self,el:&ActiveEventLoop,action:&bareline_platform::accessibility::AccessibilityAction)->bool {
        use bareline_platform::accessibility::AccessibilityAction;
        if self.compare.active_color.is_some(){match action{AccessibilityAction::SetValue{id:59_999,value}=>{if value.len()<=9&&value.chars().all(|c|c=='#'||c.is_ascii_hexdigit()){self.compare.color_field.select_all();self.compare.color_field.insert(value);self.compare.color_focus=true;self.compare_redraw();}return true;},AccessibilityAction::Focus(59_999)=>{self.compare.color_focus=true;self.compare_redraw();return true;},_=>{}}}
        let (id,invoke)=match action{AccessibilityAction::Focus(id)=>(*id,false),AccessibilityAction::Invoke(id)=>(*id,true),_=>return false};
        let Some(node)=self.compare_accessibility_nodes().into_iter().find(|node|node.id==id)else{return false;};
        if node.disabled{return true;}
        self.compare.color_focus=false;
        let Some((index,command))=self.compare.hits.iter().enumerate().find_map(|(index,(_,command))|(self.compare_accessibility_hit_id(index)==Some(id)).then_some((index,*command)))else{return false;};
        self.compare.focus=index;
        if invoke{self.compare_dispatch(el,command);}self.compare_redraw();true
    }
    /// Both indices must refer to actual loaded sources. Recovery Center owns their
    /// asynchronous opening; this bridge never compares a viewport prefix.
    pub(super) fn compare_recovery_pair(&mut self,recovered_index:usize,disk_index:usize) {
        if self.workspace.as_ref().is_none_or(|w|recovered_index>=w.editors.len()||disk_index>=w.editors.len()||recovered_index==disk_index){return;}
        if !self.compare_start_pair(recovered_index,disk_index){return;}
        if let Some(controller)=&mut self.compare.controller {
            controller.sources=[CompareSource{label:"Recovered copy".into(),origin:CompareOrigin::Recovery("Recovered copy".into())},CompareSource{label:"Current disk".into(),origin:CompareOrigin::Disk("Current disk".into())}];
        }
        self.compare_redraw();
    }
    /// Recovery/conflict controllers can supply an immutable source; adoption never
    /// copies the complete text and never gives a preview a writable file identity.
    pub(super) fn compare_snapshot_source(
        &mut self,
        snapshot: DocumentSnapshot,
        label: String,
        origin: CompareOrigin,
    ) {
        let left = self.app.active;
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        match workspace.add_snapshot_preview(&snapshot, label) {
            Ok(right) => {
                self.compare_start_pair(left, right);
                if let Some(c) = &mut self.compare.controller {
                    c.sources[1].origin = origin;
                }
            }
            Err(error) => {
                workspace.message = Some(format!("Compare source unavailable: {error:?}"))
            }
        }
    }
    fn compare_start_pair(&mut self, left: usize, right: usize)->bool {
        let Some(workspace) = &mut self.workspace else {
            return false;
        };
        if !self.views.compare_pair(workspace, left, right) {
            workspace.message=Some("Compare panes are not ready; retry after both sources finish opening".into());
            return false;
        }
        let titles = workspace.titles();
        let sources = [left, right].map(|index| CompareSource {
            label: titles[index].clone(),
            origin: CompareOrigin::OpenDocument(titles[index].clone()),
        });
        let mut controller = CompareController::new(sources[0].clone(), sources[1].clone());
        if let Some(previous) = &self.compare.controller {
            controller.set_options(previous.options().clone());
        }
        self.compare.documents = Some([
            compare_input(&workspace.editors[left]),
            compare_input(&workspace.editors[right]),
        ]);
        let result = controller.start_inputs(
            compare_input(&workspace.editors[left]),
            compare_input(&workspace.editors[right]),
            self.notify.clone(),
        );
        workspace.message = Some(
            result
                .err()
                .map_or_else(|| "Comparing…".into(), |e| format!("Compare: {e:?}")),
        );
        self.compare.controller = Some(controller);
        self.app.active = left;
        true
    }
    pub(super) fn compare_dispatch(&mut self, _el: &ActiveEventLoop, id: &str) -> bool {
        if !id.starts_with("compare.") {
            return false;
        }
        if id == "compare.lastSaved" {
            if let Some(workspace)=&mut self.workspace {
                if let Some(bareline_app::workspace::WorkspaceEditor::Paged(editor))=workspace.editors.get(self.app.active) {
                    let captured=self.compare.saved_paged.iter().find(|(handle,_)|handle.snapshot().same_document(editor.snapshot())).cloned();
                    if let Some((handle,label))=captured {
                        match workspace.add_paged_snapshot_preview(self.app.active,&handle,format!("{label} (last saved)")) {
                            Ok(right)=>{let left=self.app.active;if self.compare_start_pair(left,right){if let Some(c)=&mut self.compare.controller{c.sources[1].origin=CompareOrigin::LastSaved(label);}}},
                            Err(error)=>workspace.message=Some(error),
                        }
                    }else{workspace.message=Some("The last saved source is not available yet".into());}
                    self.compare_redraw();return true;
                }
            }
            let saved = self
                .workspace
                .as_ref()
                .and_then(|w| w.editors.get(self.app.active))
                .and_then(|e| {
                    self.compare
                        .saved
                        .iter()
                        .find(|(s, _)| s.same_document(e.snapshot()))
                })
                .cloned();
            if let Some((snapshot, label)) = saved {
                self.compare_snapshot_source(
                    snapshot,
                    format!("{label} (last saved)"),
                    CompareOrigin::LastSaved(label),
                );
            } else if let Some(workspace) = &mut self.workspace {
                workspace.message = Some("The last saved snapshot is not available yet.".into());
            }
            self.compare_redraw();
            return true;
        }
        if matches!(id, "compare.disk" | "compare.external") {
            let path = if id == "compare.disk" {
                self.platform
                    .as_ref()
                    .and_then(|p| p.open_file().ok().flatten())
            } else {
                self.workspace
                    .as_ref()
                    .and_then(|w| w.path(self.app.active).map(PathBuf::from))
            };
            if let (Some(path), Some(workspace)) = (path, &mut self.workspace)
                && let Some(left) = workspace
                    .editors
                    .get(self.app.active)
            {
                self.compare.pending_source = Some(PendingSource {
                    left: compare_input(left),
                    previous: workspace
                        .editors
                        .iter()
                        .map(compare_input)
                        .collect(),
                    origin: if id == "compare.external" {
                        CompareOrigin::ExternalConflict(path.display().to_string())
                    } else {
                        CompareOrigin::Disk(path.display().to_string())
                    },
                    path: path.clone(),
                });
                workspace.open(path);
            }
            self.compare_redraw();
            return true;
        }
        if let Some((_, key)) = COLOR_CONTROLS.iter().find(|(command, _)| *command == id) {
            self.compare.active_color = Some(key);
            self.compare.color_focus=true;
            self.compare.color_field.select_all();
            let color = self.settings.theme_color(key).map_or(0, |c| c.0);
            self.compare.color_field.insert(&format!("#{color:06X}"));
            self.compare.color_field.select_all();
            self.compare_redraw();
            return true;
        }
        if let Some(prefix)=match id{"compare.resetAdded"=>Some("diff.added"),"compare.resetRemoved"=>Some("diff.removed"),"compare.resetChanged"=>Some("diff.changed"),"compare.resetMoved"=>Some("diff.moved"),"compare.resetCurrent"=>Some("diff.current"),_=>None} {
            let mut overrides=self.settings.effective().theme_overrides;overrides.retain(|key,_|key!=prefix&&!key.starts_with(&format!("{prefix}.")));
            let result=self.settings.controller.edit("theme.overrides",bareline_settings::SettingValue::Map(overrides));if let Some(workspace)=&mut self.workspace{workspace.message=result.err();}self.compare_redraw();return true;
        }
        if matches!(
            id,
            "compare.themeLight"
                | "compare.themeDark"
                | "compare.themeSystem"
                | "compare.defaults"
                | "compare.colorblind"
                | "compare.applyColor"
        ) {
            let mut overrides = self.settings.effective().theme_overrides;
            let edit = if id == "compare.applyColor" {
                if let Some(key) = self.compare.active_color {
                    overrides.insert(key.into(), self.compare.color_field.value().into());
                }
                self.settings.controller.edit(
                    "theme.overrides",
                    bareline_settings::SettingValue::Map(overrides),
                )
            } else if id == "compare.defaults" {
                overrides.retain(|key, _| !key.starts_with("diff."));
                self.compare.color_blind = false;
                self.settings.controller.edit(
                    "theme.overrides",
                    bareline_settings::SettingValue::Map(overrides),
                )
            } else if id == "compare.colorblind" {
                self.compare.color_blind = !self.compare.color_blind;
                for (key, value) in [
                    ("diff.added", "#DCEAF7"),
                    ("diff.removed", "#FAE5CF"),
                    ("diff.added.gutter", "#2474B5"),
                    ("diff.removed.gutter", "#B96312"),
                ] {
                    if self.compare.color_blind {
                        overrides.insert(key.into(), value.into());
                    } else {
                        overrides.remove(key);
                    }
                }
                self.settings.controller.edit(
                    "theme.overrides",
                    bareline_settings::SettingValue::Map(overrides),
                )
            } else {
                self.settings.controller.edit(
                    "theme.mode",
                    bareline_settings::SettingValue::Text(
                        match id {
                            "compare.themeLight" => "light",
                            "compare.themeDark" => "dark",
                            _ => "system",
                        }
                        .into(),
                    ),
                )
            };
            self.compare
                .color_field
                .set_validation(edit.as_ref().err().cloned());
            if edit.is_ok() {
                self.compare.active_color = None;
            }
            if let Some(workspace) = &mut self.workspace {
                workspace.message = edit.err();
            }
            self.compare_redraw();
            return true;
        }
        if matches!(id, "compare.generalTab" | "compare.colorsTab") {
            self.compare.colors_tab = id == "compare.colorsTab";
            self.compare_redraw();
            return true;
        }
        if id == "compare.open" {
            if let Some(workspace) = &self.workspace {
                let left = self.app.active;
                if let Some(right) = (0..workspace.editors.len())
                    .find(|i| *i != left)
                {
                    self.compare_start_pair(left, right);
                } else if let Some(workspace) = &mut self.workspace {
                    workspace.message =
                        Some("Open two text documents, then choose Compare documents.".into());
                }
            }
            self.compare_redraw();
            return true;
        }
        if id == "compare.close" {
            if let Some(workspace) = &mut self.workspace {
                self.views.close_compare(workspace);
            }
            if let Some(renderer) = &mut self.renderer {
                self.compare.color_field.release(renderer);
            }
            let saved = std::mem::take(&mut self.compare.saved);
            let saved_paged=std::mem::take(&mut self.compare.saved_paged);
            self.compare = CompareRuntime {
                saved,
                saved_paged,
                ..Default::default()
            };
            self.compare_redraw();
            return true;
        }
        let Some(indices) = self
            .workspace
            .as_ref()
            .and_then(|w| self.compare.indices(w))
        else {
            return true;
        };
        if matches!(
            id,
            "compare.leftSource" | "compare.rightSource" | "compare.swap"
        ) {
            let mut pair = indices;
            if id == "compare.swap" {
                pair.swap(0, 1);
            } else if let Some(workspace) = &self.workspace {
                let side = usize::from(id == "compare.rightSource");
                if let Some(next) = (1..=workspace.editors.len())
                    .map(|step| (pair[side] + step) % workspace.editors.len())
                    .find(|i| *i != pair[1 - side])
                {
                    pair[side] = next;
                }
            }
            self.compare_start_pair(pair[0], pair[1]);
            self.compare_redraw();
            return true;
        }
        let Some(workspace) = &mut self.workspace else {
            return true;
        };
        let snapshots = indices.map(|i| workspace.editors[i].snapshot().clone());
        let inputs = indices.map(|i| compare_input(&workspace.editors[i]));
        let Some(controller) = &mut self.compare.controller else {
            return true;
        };
        match id {
            "compare.next" | "compare.previous" => {
                if let Some(hunk) = controller.navigate(id == "compare.previous") {
                    self.views
                        .compare_navigate(workspace, hunk.left.start, hunk.right.start);
                }
            }
            "compare.recompare" => {
                let _ = controller.start_inputs(
                    inputs[0].clone(),
                    inputs[1].clone(),
                    self.notify.clone(),
                );
            }
            "compare.cancel" => {controller.cancel();self.compare.pending_merge=None;},
            "compare.options" => {
                self.compare.options_open = !self.compare.options_open;
                if !self.compare.options_open
                    && let Some(renderer) = &mut self.renderer
                {
                    self.compare.color_field.release(renderer);
                }
                self.compare.focus = 0;
                self.compare.colors_tab = true;
            }
            "compare.pauseAutomatic" => controller.pause_automatic = !controller.pause_automatic,
            "compare.syncHorizontal" => controller.sync_horizontal = !controller.sync_horizontal,
            "compare.copyLeftToRight" | "compare.copyRightToLeft" => {
                let side = usize::from(id == "compare.copyLeftToRight");
                let direction = if side == 1 {
                    Direction::LeftToRight
                } else {
                    Direction::RightToLeft
                };
                if inputs.iter().any(|input|!matches!(input,CompareInput::Resident(_))) {
                    if self.compare.pending_merge.is_some(){return true;}
                    controller.visible_input_hunks(&inputs[0],&inputs[1]);
                    let Some(hunk)=controller.current_hunk().cloned()else{return true;};
                    if workspace.editors[indices[side]].read_only()||workspace.editors[indices[side]].busy(){workspace.message=Some("Destination is read-only or busy".into());return true;}
                    let options=controller.options().clone();let cancel=bareline_diff::CancelToken::default();let worker_cancel=cancel.clone();let captured=inputs[side].clone();let notify=self.notify.clone();let (send,result)=std::sync::mpsc::sync_channel(1);
                    let spawned=std::thread::Builder::new().name("compare-merge".into()).spawn(move||{let result=bareline_app::compare::prepare_input_merge(&inputs[0],&inputs[1],&hunk,direction,&options,MergePolicy::PreserveIgnoredDestination,1024*1024,&worker_cancel);let _=send.send(result);notify();});
                    if spawned.is_ok(){controller.invalidate();controller.state=CompareState::Running;self.compare.pending_merge=Some(PendingMerge{source:captured,cancel,result});workspace.message=Some("Preparing difference · Cancel stops staging".into());}else{workspace.message=Some("Could not start merge worker; retry".into());}
                    return true;
                }
                match controller.merge(
                    direction,
                    &snapshots[0],
                    &snapshots[1],
                    1024 * 1024,
                    MergePolicy::PreserveIgnoredDestination,
                ) {
                    Ok(transaction) => {
                        workspace.message = Some(
                            match workspace.editors[indices[side]]
                                .apply_prepared(&snapshots[side], transaction)
                            {
                                Ok(()) => {
                                    "Difference applied · Undo restores the destination".into()
                                }
                                Err(e) => e.into(),
                            },
                        );
                    }
                    Err(error) => {
                        workspace.message =
                            Some(format!("Difference could not be applied: {error:?}"))
                    }
                }
            }
            _ => {
                let mut options = controller.options().clone();
                match id {
                    "compare.whitespace" => {
                        options.whitespace = match options.whitespace {
                            Whitespace::Significant => Whitespace::TrimEdges,
                            Whitespace::TrimEdges => Whitespace::IgnoreAll,
                            Whitespace::IgnoreAll => Whitespace::Significant,
                        }
                    }
                    "compare.trimEdges" => {
                        options.whitespace = if options.whitespace == Whitespace::TrimEdges {
                            Whitespace::Significant
                        } else {
                            Whitespace::TrimEdges
                        }
                    }
                    "compare.ignoreWhitespace" => {
                        options.whitespace = if options.whitespace == Whitespace::IgnoreAll {
                            Whitespace::Significant
                        } else {
                            Whitespace::IgnoreAll
                        }
                    }
                    "compare.ignoreCase" => options.ignore_case = !options.ignore_case,
                    "compare.ignoreBlank" => {
                        options.ignore_blank_lines = !options.ignore_blank_lines
                    }
                    "compare.ignoreEol" => options.ignore_eol_style = !options.ignore_eol_style,
                    "compare.ignoreBom" => {
                        options.ignore_encoding_bom = !options.ignore_encoding_bom
                    }
                    "compare.normalizeTabs" => options.normalize_tabs = !options.normalize_tabs,
                    _ => return true,
                }
                controller.set_options(options);
                let _ = controller.start_inputs(
                    inputs[0].clone(),
                    inputs[1].clone(),
                    self.notify.clone(),
                );
            }
        }
        self.compare_redraw();
        true
    }
    fn compare_redraw(&self) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(super) fn compare_pump(&mut self, _el: &ActiveEventLoop) {
        let Some(workspace) = &mut self.workspace else {
            return;
        };
        if let Some(pending)=&self.compare.pending_merge {
            let received=match pending.result.try_recv(){Ok(result)=>Some(result),Err(std::sync::mpsc::TryRecvError::Empty)=>None,Err(_)=>Some(Err(bareline_diff::ApplyError::Unavailable))};
            if let Some(received)=received {
                let pending=self.compare.pending_merge.take().unwrap();
                let result=received.map_err(|e|format!("Merge could not be staged: {e:?}. Select a difference below 1 MiB or retry."));
                let message=match result {
                    Ok(transaction)=>match workspace.editors.iter_mut().find(|e|compare_input(e).same_document(&pending.source)) {
                        Some(editor)=>match (editor,&pending.source){(bareline_app::workspace::WorkspaceEditor::Resident(editor),CompareInput::Resident(source))=>editor.apply_prepared(source,transaction).map_err(String::from),(bareline_app::workspace::WorkspaceEditor::Paged(editor),CompareInput::Paged(source))=>editor.apply_prepared(source.snapshot(),transaction),(bareline_app::workspace::WorkspaceEditor::Paged(editor),CompareInput::CapturedPaged(source,_))=>editor.apply_prepared(source,transaction),_=>Err("Destination changed".into())}.map_or_else(|e|e,|()|"Difference queued as one undoable edit; destination remains unsaved".into()),
                        None=>"Destination closed; difference was not applied".into(),
                    },Err(e)=>e,
                };
                workspace.message=Some(message);if let Some(controller)=&mut self.compare.controller{controller.invalidate();}
            }
        }
        self.compare.saved_paged.retain(|(saved,_)|workspace.editors.iter().any(|e|match e{bareline_app::workspace::WorkspaceEditor::Paged(p)=>p.snapshot().same_document(saved.snapshot()),_=>false}));
        let paged_titles=workspace.titles();
        for (index,editor) in workspace.editors.iter().enumerate(){if let bareline_app::workspace::WorkspaceEditor::Paged(paged)=editor{if workspace.path(index).is_some()&&!paged.dirty()&&!paged.busy(){if let Some((saved,_))=self.compare.saved_paged.iter_mut().find(|(saved,_)|saved.snapshot().same_document(paged.snapshot())){*saved=paged.read_handle();}else if self.compare.saved_paged.len()<4096{self.compare.saved_paged.push((paged.read_handle(),paged_titles[index].clone()));}}}}
        self.compare.saved.retain(|(saved, _)| {
            workspace
                .editors
                .iter()
                .any(|e| e.snapshot().same_document(saved))
        });
        let titles = workspace.titles();
        for (index, editor) in workspace.editors.iter().enumerate() {
            if !editor.paged()
                && editor.snapshot().is_complete()
                && !editor.dirty()
                && workspace.path(index).is_some()
            {
                if let Some((saved, _)) = self
                    .compare
                    .saved
                    .iter_mut()
                    .find(|(saved, _)| saved.same_document(editor.snapshot()))
                {
                    *saved = editor.snapshot().clone();
                } else if self.compare.saved.len() < 4096 {
                    self.compare
                        .saved
                        .push((editor.snapshot().clone(), titles[index].clone()));
                }
            }
        }
        if let Some(pending) = &self.compare.pending_source {
            let right = workspace
                .editors
                .iter()
                .enumerate()
                .find(|(index, e)| {
                    workspace.path(*index) == Some(pending.path.as_path())
                        && (e.paged() || e.snapshot().is_complete())
                        && !pending
                            .previous
                            .iter()
                            .any(|old| old.same_document(&compare_input(e)))
                })
                .map(|(i, _)| i);
            let left = workspace
                .editors
                .iter()
                .position(|e| compare_input(e).same_document(&pending.left));
            if let (Some(left), Some(right)) = (left, right) {
                let pending = self.compare.pending_source.take().unwrap();
                workspace.editors[right].set_read_only(true);
                if self.compare_start_pair(left, right) {
                    if let Some(c) = &mut self.compare.controller {
                        c.sources[1].origin = pending.origin;
                    }
                }
                self.compare_redraw();
                return;
            }
            if !workspace.path_loading(&pending.path) && !workspace.io_busy() {
                self.compare.pending_source = None;
            }
        }
        let Some(indices) = self.compare.indices(workspace) else {
            if self.compare.controller.is_some() {
                self.views.close_compare(workspace);
                self.compare = Default::default();
            }
            return;
        };
        let snapshots = indices.map(|i| compare_input(&workspace.editors[i]));
        let controller = self.compare.controller.as_mut().unwrap();
        let mut changed = controller.poll_inputs(&snapshots[0], &snapshots[1]);
        controller.visible_input_hunks(&snapshots[0], &snapshots[1]);
        if controller.state == CompareState::Stale
            && !controller.pause_automatic
            && indices.iter().all(|i| !workspace.editors[*i].busy())
        {
            let _ = controller.start_inputs_debounced(
                snapshots[0].clone(),
                snapshots[1].clone(),
                self.notify.clone(),
            );
            changed = true;
        }
        if changed {
            self.views
                .set_compare_alignment(controller.alignment().cloned());
            workspace.message = Some(controller.status_text());
            self.compare_redraw();
        }
    }
    pub(super) fn compare_event(&mut self, el: &ActiveEventLoop, event: &WindowEvent) -> bool {
        if self.compare.controller.is_none() {
            return false;
        }
        match event {
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                let pointer = self.editor_pointer();
                if self.compare.active_color.is_some()&&self.compare.color_bounds.is_some_and(|bounds|bounds.contains(pointer)) {self.compare.color_focus=true;if let Some(renderer)=&self.renderer{let _=self.compare.color_field.click(renderer,pointer,self.modifiers.shift_key());}self.compare_redraw();return true;}
                if !self.compare.options_open {
                    if let Some((side,bounds))=self.compare.overview.iter().enumerate().find_map(|(i,r)|r.filter(|r|r.contains(pointer)).map(|r|(i,r))) {
                        if let Some(workspace)=&mut self.workspace {
                            if let Some(indices)=self.compare.indices(workspace) {
                                let length=compare_input(&workspace.editors[indices[side]]).len();
                                let offset=bareline_document::TextOffset((((pointer.y-bounds.y)/bounds.height).clamp(0.0,1.0) as f64*length as f64) as usize);
                                if let Some(h)=self.compare.controller.as_mut().and_then(|c|c.navigate_offset(side==1,offset)) {self.views.compare_navigate(workspace,h.left.start,h.right.start);}
                            }
                        }
                        self.compare_redraw();return true;
                    }
                }
                if let Some((index, (_, id))) = self
                    .compare
                    .hits
                    .iter()
                    .enumerate()
                    .find(|(_, (r, _))| r.contains(pointer))
                {
                    let id = *id;
                    self.compare.focus = index;
                    return self.compare_dispatch(el, id);
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if self.compare.active_color.is_some() {
                    match &event.logical_key {
                        Key::Named(NamedKey::Tab)=>{self.compare.color_focus=!self.compare.color_focus;self.compare.focus=0;},
                        Key::Named(NamedKey::Enter) => {
                            return self.compare_dispatch(el, "compare.applyColor");
                        }
                        Key::Named(NamedKey::Escape) => self.compare.active_color = None,
                        Key::Named(NamedKey::Backspace) if self.compare.color_focus => {
                            self.compare.color_field.delete(false);
                        }
                        Key::Named(NamedKey::Delete) if self.compare.color_focus => {
                            self.compare.color_field.delete(true);
                        }
                        Key::Named(NamedKey::ArrowLeft) if self.compare.color_focus => self
                            .compare
                            .color_field
                            .horizontal(false, self.modifiers.shift_key()),
                        Key::Named(NamedKey::ArrowRight) if self.compare.color_focus => self
                            .compare
                            .color_field
                            .horizontal(true, self.modifiers.shift_key()),
                        Key::Character(value)
                            if self.modifiers.control_key() && value.eq_ignore_ascii_case("a") =>
                        {
                            self.compare.color_field.select_all()
                        }
                        _ => {
                            if self.compare.color_focus && let Some(value) = &event.text
                                && value.chars().all(|c| c == '#' || c.is_ascii_hexdigit())
                            {
                                self.compare.color_field.insert(value);
                            }
                        }
                    }
                    self.compare_redraw();
                    return true;
                }
                match &event.logical_key {
                    Key::Named(NamedKey::Tab | NamedKey::ArrowDown | NamedKey::ArrowUp)
                        if self.compare.options_open =>
                    {
                        let first = 0;
                        let count = self.compare.hits.len().saturating_sub(first);
                        if count > 0 {
                            let backward = self.modifiers.shift_key()
                                || event.logical_key == Key::Named(NamedKey::ArrowUp);
                            let current = self.compare.focus.saturating_sub(first).min(count - 1);
                            self.compare.focus =
                                first + (current + if backward { count - 1 } else { 1 }) % count;
                            self.compare_redraw();
                        }
                        return true;
                    }
                    Key::Named(NamedKey::Enter | NamedKey::Space) if self.compare.options_open => {
                        if let Some((_, id)) = self.compare.hits.get(self.compare.focus) {
                            return self.compare_dispatch(el, id);
                        }
                        return true;
                    }
                    Key::Named(NamedKey::Escape) if self.compare.options_open => {
                        self.compare.options_open = false;
                        self.compare_redraw();
                        return true;
                    }
                    Key::Named(NamedKey::F7) => {
                        return self.compare_dispatch(
                            el,
                            if self.modifiers.shift_key() {
                                "compare.previous"
                            } else {
                                "compare.next"
                            },
                        );
                    }
                    _ => {}
                }
            }
            _ => {}
        }
        self.compare.options_open
            && matches!(
                event,
                WindowEvent::KeyboardInput { .. }
                    | WindowEvent::MouseInput { .. }
                    | WindowEvent::MouseWheel { .. }
            )
    }
}
impl CompareRuntime {
    /// Called after normal panes draw, before their operations are translated to window coordinates.
    #[allow(clippy::too_many_arguments)] // Disjoint borrowed render controllers keep ownership in the shell.
    pub(super) fn draw(
        &mut self,
        workspace: &mut Workspace,
        views: &mut views::ViewsRuntime,
        settings: &settings::SettingsRuntime,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<(), LayoutError> {
        let Some(indices) = self.indices(workspace) else {
            return Ok(());
        };
        let inputs = indices.map(|i| compare_input(&workspace.editors[i]));
        let bases=views.compare_viewport_starts(workspace);
        let geometry = views.compare_geometry();
        let line_height =
            (settings.effective().editor_font_size_pt.clamp(6.0, 72.0) * 96.0 / 72.0 * 1.2) as f32;
        let theme = settings.ui_theme();
        self.color_bounds=None;
        let colors = [
            "diff.added",
            "diff.removed",
            "diff.changed",
            "diff.moved",
            "diff.current",
        ]
        .map(|key| settings.theme_color(key).unwrap_or(theme.focus));
        let Some(controller) = &mut self.controller else {
            return Ok(());
        };
        let current = controller.current_hunk().map(|h| h.stable_id);
        let hunks = controller.visible_input_hunks(&inputs[0], &inputs[1]);
        let mut painted = Vec::with_capacity(ops.len() + hunks.len().min(256) * 4);
        for op in ops.drain(..) {
            if let DrawOp::Layout { origin, layout, .. } = &op
                && let Some(side) = geometry
                    .iter()
                    .position(|r| r.is_some_and(|r| r.contains(*origin)))
            {
                let bounds = geometry[side].unwrap();
                if let Some(local_line) = views.compare_layout_range(workspace,side,*layout) {
                    let line=bareline_document::TextOffset(local_line.start.0+bases[side])..bareline_document::TextOffset(local_line.end.0+bases[side]);
                    for hunk in hunks {
                        let range = if side == 0 { &hunk.left } else { &hunk.right };
                        if range.start < line.end && range.end > line.start {
                            let color = colors[match hunk.kind {
                                DiffKind::Added => 0,
                                DiffKind::Removed => 1,
                                DiffKind::MovedAligned => 3,
                                _ => 2,
                            }];
                            painted.push(DrawOp::Fill(
                                rect(bounds.x + 49.0, origin.y, bounds.width - 65.0, line_height),
                                color,
                            ));
                            text(
                                &mut painted,
                                bounds.x + 35.0,
                                origin.y,
                                match hunk.kind {
                                    DiffKind::Added => "+",
                                    DiffKind::Removed => "−",
                                    DiffKind::MovedAligned => "↕",
                                    _ => "~",
                                },
                                12.0,
                                settings.theme_color(match hunk.kind{DiffKind::Added=>"diff.added.gutter",DiffKind::Removed=>"diff.removed.gutter",DiffKind::MovedAligned=>"diff.moved.gutter",_=>"diff.changed.gutter"}).unwrap_or(theme.text),
                            );
                            if Some(hunk.stable_id) == current {
                                painted.push(DrawOp::Stroke(
                                    rect(bounds.x + 1.0, origin.y, bounds.width - 3.0, line_height),
                                    colors[4],
                                    1.0,
                                ));
                            }
                            for span in &hunk.intraline {
                                let span = if side == 0 { &span.left } else { &span.right };
                                let start = span.start.0.max(line.start.0);
                                let end = span.end.0.min(line.end.0);
                                if start < end
                                    && let Ok(rectangles) = renderer.range_rects(
                                        *layout,
                                        start - line.start.0..end - line.start.0,
                                    )
                                {
                                    for r in rectangles {
                                        painted.push(DrawOp::Stroke(
                                            rect(origin.x + r.x, origin.y + r.y, r.width, r.height),
                                            colors[4],
                                            1.0,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }
            painted.push(op);
        }
        *ops = painted;
        self.overview=[None,None];
        for side in 0..2 {
            if let Some(pane)=geometry[side] {
                let track=rect(pane.x+pane.width-10.0,pane.y+TAB_HEIGHT,8.0,(pane.height-TAB_HEIGHT).max(1.0));
                self.overview[side]=Some(track);
                ops.push(DrawOp::Fill(track,theme.chrome));
                let length=inputs[side].len().max(1) as f64;
                for h in hunks.iter().take(4096) {
                    let range=if side==0{&h.left}else{&h.right};
                    let kind=match h.kind{DiffKind::Added=>0,DiffKind::Removed=>1,DiffKind::MovedAligned=>3,_=>2};
                    let key=["diff.added.overview","diff.removed.overview","diff.changed.overview","diff.moved.overview"][kind];
                    let y=track.y+(range.start.0 as f64/length*track.height as f64) as f32;
                    let size=((range.end.0.saturating_sub(range.start.0) as f64/length*track.height as f64) as f32).max(3.0).min(track.height);
                    let marker=rect(track.x,y.min(track.y+track.height-size),track.width,size);
                    ops.push(DrawOp::Fill(marker,settings.theme_color(key).unwrap_or(colors[kind])));
                    if Some(h.stable_id)==current{ops.push(DrawOp::Stroke(marker,colors[4],1.0));}
                }
            }
        }
        self.hits.clear();
        ops.push(DrawOp::Fill(
            rect(0.0, TAB_HEIGHT, width, 44.0),
            theme.chrome,
        ));
        let source_width = ((width - 710.0) / 2.0).max(72.0);
        let mut x = 8.0;
        let mut button = |label: String, id: &'static str, w: f32| {
            let bounds = rect(x, TAB_HEIGHT + 6.0, w, 32.0);
            x += w + 6.0;
            ops.push(DrawOp::StrokeRounded(bounds, theme.border, 4.0, 1.0));
            ops.push(DrawOp::PushClip(bounds));
            text(
                ops,
                bounds.x + 9.0,
                bounds.y + 8.0,
                &label,
                12.0,
                theme.text,
            );
            ops.push(DrawOp::PopClip);
            self.hits.push((bounds, id));
        };
        button(
            controller.sources[0].label.clone(),
            "compare.leftSource",
            source_width,
        );
        button("⇄".into(), "compare.swap", 32.0);
        button(
            controller.sources[1].label.clone(),
            "compare.rightSource",
            source_width,
        );
        button(
            match controller.options().whitespace {
                Whitespace::Significant => "Whitespace significant",
                Whitespace::TrimEdges => "Trim edges",
                Whitespace::IgnoreAll => "Ignore whitespace",
            }
            .into(),
            "compare.whitespace",
            155.0,
        );
        button("Options".into(), "compare.options", 66.0);
        button("←".into(), "compare.previous", 32.0);
        button("→".into(), "compare.next", 32.0);
        let (index, total) = controller.counter();
        button(format!("{index} / {total}"), "compare.next", 62.0);
        button(
            if controller.state == CompareState::Running {
                "Cancel"
            } else {
                "Recompare"
            }
            .into(),
            if controller.state == CompareState::Running {
                "compare.cancel"
            } else {
                "compare.recompare"
            },
            88.0,
        );
        button("Close".into(), "compare.close", 60.0);
        if self.options_open {
            self.draw_options(settings, renderer, width, height, ops)?;
        }
        Ok(())
    }
    fn draw_options(
        &mut self,
        settings: &settings::SettingsRuntime,
        renderer: &mut impl TextBackend,
        width: f32,
        height: f32,
        ops: &mut Vec<DrawOp>,
    ) -> Result<(), LayoutError> {
        let theme = settings.ui_theme();
        let scale = ((height - 24.0) / 780.0)
            .min((width - 24.0) / 550.0)
            .clamp(0.45, 1.0);
        let panel = rect(
            (width - 550.0 * scale) / 2.0,
            (height - 780.0 * scale) / 2.0,
            550.0 * scale,
            780.0 * scale,
        );
        let r = |x: f32, y: f32, w: f32, h: f32| {
            rect(
                panel.x + x * scale,
                panel.y + y * scale,
                w * scale,
                h * scale,
            )
        };
        let label = |ops: &mut Vec<DrawOp>, x: f32, y: f32, value: &str, size: f32| {
            text(
                ops,
                panel.x + x * scale,
                panel.y + y * scale,
                value,
                size * scale,
                theme.text,
            )
        };
        ops.push(DrawOp::FillRounded(panel, theme.elevated, 10.0));
        ops.push(DrawOp::StrokeRounded(panel, theme.border, 10.0, 1.0));
        self.hits.clear();
        label(ops, 24.0, 24.0, "Compare options", 24.0);
        option_button(
            ops,
            &mut self.hits,
            r(24.0, 65.0, 100.0, 38.0),
            "General",
            "compare.generalTab",
            theme,
        );
        option_button(
            ops,
            &mut self.hits,
            r(124.0, 65.0, 100.0, 38.0),
            "Colors",
            "compare.colorsTab",
            theme,
        );
        ops.push(DrawOp::Fill(
            r(
                if self.colors_tab { 124.0 } else { 24.0 },
                101.0,
                100.0,
                3.0,
            ),
            theme.focus,
        ));
        let options = self.controller.as_ref().unwrap().options();
        let flags = [
            (
                "Ignore leading/trailing whitespace",
                options.whitespace == Whitespace::TrimEdges,
                "compare.trimEdges",
            ),
            (
                "Ignore all whitespace",
                options.whitespace == Whitespace::IgnoreAll,
                "compare.ignoreWhitespace",
            ),
            (
                "Ignore blank lines",
                options.ignore_blank_lines,
                "compare.ignoreBlank",
            ),
            ("Ignore case", options.ignore_case, "compare.ignoreCase"),
            (
                "Ignore EOL style",
                options.ignore_eol_style,
                "compare.ignoreEol",
            ),
        ];
        if self.colors_tab {
            for (i, (name, id)) in [
                ("Light", "compare.themeLight"),
                ("Dark", "compare.themeDark"),
                ("System", "compare.themeSystem"),
            ]
            .into_iter()
            .enumerate()
            {
                option_button(
                    ops,
                    &mut self.hits,
                    r(54.0 + i as f32 * 148.0, 123.0, 148.0, 38.0),
                    name,
                    id,
                    theme,
                );
            }
            for (row, (name, symbol)) in [
                ("Added", "+"),
                ("Removed", "−"),
                ("Changed", "~"),
                ("Moved", "↕"),
                ("Current", "●"),
            ]
            .into_iter()
            .enumerate()
            {
                let y = 192.0 + row as f32 * 52.0;
                label(ops, 28.0, y + 10.0, name, 14.0);
                for (column, (caption, x)) in
                    [("Background", 120.0), ("Accent", 265.0), ("Gutter", 380.0)]
                        .into_iter()
                        .enumerate()
                {
                    label(ops, x, y + 10.0, caption, 13.0);
                    let bounds = r(x + if column == 0 { 90.0 } else { 58.0 }, y, 30.0, 30.0);
                    let (id, key) = COLOR_CONTROLS[row * 3 + column];
                    let color = settings.theme_color(key).unwrap_or(theme.focus);
                    ops.push(DrawOp::FillRounded(bounds, color, 5.0));
                    ops.push(DrawOp::StrokeRounded(bounds, theme.border, 5.0, 1.0));
                    if column == 2 {
                        text(
                            ops,
                            bounds.x + 8.0 * scale,
                            bounds.y + 4.0 * scale,
                            symbol,
                            18.0 * scale,
                            theme.text,
                        );
                    }
                    self.hits.push((bounds, id));
                }
                option_button(ops,&mut self.hits,r(482.0,y,30.0,30.0),"↶",["compare.resetAdded","compare.resetRemoved","compare.resetChanged","compare.resetMoved","compare.resetCurrent"][row],theme);
            }
            ops.push(DrawOp::Fill(r(24.0, 458.0, 502.0, 1.0), theme.border));
            option_button(
                ops,
                &mut self.hits,
                r(28.0, 476.0, 492.0, 30.0),
                &format!(
                    "{}  Color-blind palette (blue / orange)",
                    if self.color_blind { "☑" } else { "☐" }
                ),
                "compare.colorblind",
                theme,
            );
        }
        let start = if self.colors_tab { 520.0 } else { 135.0 };
        for (index, (name, checked, id)) in flags.iter().enumerate() {
            let bounds = r(28.0, start + index as f32 * 31.0, 490.0, 29.0);
            option_button(
                ops,
                &mut self.hits,
                bounds,
                &format!("{}  {name}", if *checked { "☑" } else { "☐" }),
                id,
                theme,
            );
        }
        if !self.colors_tab {
            let controller = self.controller.as_ref().unwrap();
            for (index, (name, checked, id)) in [
                (
                    "Ignore encoding BOM",
                    controller.options().ignore_encoding_bom,
                    "compare.ignoreBom",
                ),
                (
                    "Normalize tabs",
                    controller.options().normalize_tabs,
                    "compare.normalizeTabs",
                ),
                (
                    "Pause automatic recompare",
                    controller.pause_automatic,
                    "compare.pauseAutomatic",
                ),
                ("Copy left → right", false, "compare.copyLeftToRight"),
                ("Copy right → left", false, "compare.copyRightToLeft"),
            ]
            .into_iter()
            .enumerate()
            {
                option_button(
                    ops,
                    &mut self.hits,
                    r(28.0, 305.0 + index as f32 * 38.0, 490.0, 32.0),
                    &format!("{}  {name}", if checked { "☑" } else { "☐" }),
                    id,
                    theme,
                );
            }
        }
        option_button(
            ops,
            &mut self.hits,
            r(228.0, 713.0, 183.0, 44.0),
            "Use theme defaults",
            "compare.defaults",
            theme,
        );
        option_button(
            ops,
            &mut self.hits,
            r(425.0, 713.0, 101.0, 44.0),
            "Done",
            "compare.options",
            theme,
        );
        if let Some(key) = self.active_color {
            self.hits.clear();
            let popup = r(45.0, 300.0, 460.0, 150.0);
            ops.push(DrawOp::FillRounded(popup, theme.elevated, 8.0));
            ops.push(DrawOp::StrokeRounded(popup, theme.focus, 8.0, 2.0));
            label(ops, 62.0, 315.0, &format!("Color: {key}"), 16.0);
            self.color_bounds=Some(r(62.0,349.0,265.0,38.0));
            self.color_field.draw_with_theme(
                renderer,
                r(62.0, 349.0, 265.0, 38.0),
                self.color_focus,
                theme,
                ops,
            )?;
            option_button(
                ops,
                &mut self.hits,
                r(342.0, 349.0, 135.0, 38.0),
                "Apply",
                "compare.applyColor",
                theme,
            );
            label(
                ops,
                62.0,
                402.0,
                self.color_field
                    .validation()
                    .unwrap_or("#RRGGBB or #RRGGBBAA · Enter to apply"),
                13.0,
            );
        }
        if let Some((bounds, _)) = self.hits.get(self.focus) {
            ops.push(DrawOp::StrokeRounded(*bounds, theme.focus, 4.0, 1.0));
        }
        Ok(())
    }
}
fn option_button(
    ops: &mut Vec<DrawOp>,
    hits: &mut Vec<(Rect, &'static str)>,
    bounds: Rect,
    label: &str,
    id: &'static str,
    theme: bareline_ui::theme::UiTheme,
) {
    ops.push(DrawOp::StrokeRounded(bounds, theme.border, 4.0, 1.0));
    text(
        ops,
        bounds.x + 8.0,
        bounds.y + (bounds.height - 14.0) / 2.0,
        label,
        12.0,
        theme.text,
    );
    hits.push((bounds, id));
}

#[cfg(test)]
pub(super) fn accessibility_test_setup(shell:&mut Shell,scenario:&str) {
    register(&mut shell.app.commands);
    shell.compare=CompareRuntime::default();
    if scenario=="closed"{return;}
    shell.views=views::ViewsRuntime::default();
    let notify:std::sync::Arc<dyn Fn()+Send+Sync>=std::sync::Arc::new(||{});
    let mut workspace=Workspace::new(notify,std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem)).expect("compare fixture workspace");
    workspace.new_document().unwrap();workspace.new_document().unwrap();
    workspace.editors[0].enqueue(Input::Insert("anchor\nleft 🙂\nend\n".into()));
    workspace.editors[1].enqueue(Input::Insert("anchor\nright 🙂\nend\n".into()));
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(5);
    while workspace.editors.iter().any(|editor|editor.busy()){assert!(std::time::Instant::now()<deadline,"fixture editor completion");workspace.pump();std::thread::yield_now();}
    shell.workspace=Some(workspace);shell.app.active=0;
    assert!(shell.compare_start_pair(0,1));
    {
        let workspace=shell.workspace.as_ref().unwrap();let inputs=[compare_input(&workspace.editors[0]),compare_input(&workspace.editors[1])];
        let controller=shell.compare.controller.as_mut().unwrap();
        while !controller.poll_inputs(&inputs[0],&inputs[1]){assert!(std::time::Instant::now()<deadline,"fixture comparison completion");std::thread::yield_now();}
        assert!(matches!(controller.state,CompareState::Exact|CompareState::Coarse(_)));
    }
    match scenario {
        "open"|"populated"=>{},
        "options"=>{shell.compare.options_open=true;shell.compare.colors_tab=false;},
        "colors"=>{shell.compare.options_open=true;shell.compare.colors_tab=true;},
        "focus"=>{shell.compare.options_open=true;shell.compare.colors_tab=true;shell.compare.focus=2;},
        "value"=>{shell.compare.options_open=true;shell.compare.colors_tab=true;shell.compare.active_color=Some("diff.added");shell.compare.color_focus=true;shell.compare.color_field.insert("#123456");},
        _=>panic!("unknown compare accessibility fixture: {scenario}"),
    }
    let mut renderer=bareline_renderer_recording::RecordingBackend::default();let mut ops=Vec::new();
    let workspace=shell.workspace.as_mut().unwrap();
    shell.views.draw(workspace,&mut shell.app,&mut renderer,1000.0,800.0,&mut ops,shell.notify.clone()).unwrap();
    shell.compare.draw(workspace,&mut shell.views,&shell.settings,&mut renderer,1000.0,800.0,&mut ops).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_compare_merges_through_document_actor_and_undo() {
        let notify = std::sync::Arc::new(|| {});
        let mut workspace = Workspace::new(
            notify.clone(),
            std::sync::Arc::new(bareline_platform_windows::WindowsFileSystem),
        )
        .unwrap();
        workspace.new_document().unwrap();
        workspace.new_document().unwrap();
        workspace.editors[0].enqueue(Input::Insert("anchor\nleft\n".into()));
        workspace.editors[1].enqueue(Input::Insert("anchor\nright\n".into()));
        let deadline = Instant::now() + Duration::from_secs(5);
        while workspace.editors.iter().any(|e| e.busy()) {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let snapshots = [
            workspace.editors[0].snapshot().clone(),
            workspace.editors[1].snapshot().clone(),
        ];
        let source = |label: &str| CompareSource {
            label: label.into(),
            origin: CompareOrigin::OpenDocument(label.into()),
        };
        let mut runtime = CompareRuntime {
            controller: Some(CompareController::new(source("left"), source("right"))),
            documents: Some(snapshots.clone().map(CompareInput::Resident)),
            ..Default::default()
        };
        let mut views = views::ViewsRuntime::default();
        assert!(views.compare_pair(&mut workspace, 0, 1));
        let controller = runtime.controller.as_mut().unwrap();
        controller
            .start(snapshots[0].clone(), snapshots[1].clone(), notify)
            .unwrap();
        while !controller.poll(&snapshots[0], &snapshots[1]) {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        let transaction = controller
            .merge(
                Direction::LeftToRight,
                &snapshots[0],
                &snapshots[1],
                4096,
                MergePolicy::PreserveIgnoredDestination,
            )
            .unwrap();
        workspace.editors[1]
            .apply_prepared(&snapshots[1], transaction)
            .unwrap();
        while workspace.editors[1].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let actual = workspace.editors[1].snapshot();
        assert_eq!(
            actual
                .read(
                    bareline_document::TextOffset(0)..bareline_document::TextOffset(actual.len()),
                    4096
                )
                .unwrap(),
            "anchor\nleft\n"
        );
        workspace.editors[1].enqueue(Input::Undo);
        while workspace.editors[1].busy() {
            assert!(Instant::now() < deadline);
            workspace.pump();
            std::thread::yield_now();
        }
        let actual = workspace.editors[1].snapshot();
        assert_eq!(
            actual
                .read(
                    bareline_document::TextOffset(0)..bareline_document::TextOffset(actual.len()),
                    4096
                )
                .unwrap(),
            "anchor\nright\n"
        );
        assert_eq!(runtime.indices(&workspace), Some([0, 1]));
    }
}
