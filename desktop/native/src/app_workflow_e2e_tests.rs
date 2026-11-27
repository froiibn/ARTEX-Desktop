//! Windows headless egui + real NativeApp/Service workflow regression.
//! This does not qualify a native OS window, vault, provider, or external host.
use super::*;

struct Fixture {
    app: NativeApp,
    ctx: egui::Context,
    _dir: tempfile::TempDir,
    path: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        Self::configured(|_| ServiceConfig::default())
    }
    fn configured(config: impl FnOnce(&std::path::Path) -> ServiceConfig) -> Self {
        Self::with_workspace(config, |_| false)
    }
    fn configured_seeded(
        config: impl FnOnce(&std::path::Path) -> ServiceConfig,
        seed: impl FnOnce(&std::path::Path),
    ) -> Self {
        Self::with_workspace(config, |path| { seed(path); true })
    }
    fn with_workspace(
        config: impl FnOnce(&std::path::Path) -> ServiceConfig,
        initialize: impl FnOnce(&std::path::Path) -> bool,
    ) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let profile = dir.path().join("profile");
        let path = dir.path().join("workspace");
        let existing = initialize(&path);
        let instance = Arc::new(ProfileInstanceLock::acquire(&profile).unwrap());
        let ctx = egui::Context::default();
        configure(&ctx);
        ctx.style_mut(|style| style.animation_time = 0.);
        // A release frame must stop after rendering the actual UI once, so the
        // test can observe its submitted Prepare before the next app poll.
        ctx.options_mut(|options| options.max_passes = 1.try_into().unwrap());
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
        // Default fixture has no host. Provider fixtures supply a bounded memory
        // secret wrapper and loopback listeners; no OS secret is resolved.
        let service = Service::spawn_with_profile(config(&profile), &profile).unwrap();
        let handle = service.handle();
        let power = NativePower::start(handle.clone(), wake.clone());
        let launch = instance.start_launch_listener(wake.clone()).unwrap();
        let tray = NativeTray::dormant_for_test(wake);
        let mut forms = Forms::default();
        forms.set_allowed_operations(Forms::all_known_operations());
        let app = NativeApp {
            service: Some(service),
            handle,
            power,
            suspended: false,
            last_workspace: None,
            last_read_only: false,
            power_warning_recorded: false,
            _instance: instance,
            launch,
            tray,
            background: false,
            tray_warning_recorded: false,
            incoming_workspace: None,
            next_workspace: None,
            section: Section::Overview,
            selected: BTreeMap::new(),
            panes: Panes::default(),
            forms,
            source_operations: SourceOperations::default(),
            graph: Graph::default(),
            pending: HashMap::new(),
            recent: Vec::new(),
            profile,
            workspace_name: "ARTEX".into(),
            search: String::new(),
            error: None,
            notices: VecDeque::new(),
            analysis_status: crate::analysis_status::StatusTracker::default(),
            analysis_notice: None,
            close_confirmation: false,
            exit_after_close: false,
            allow_exit: false,
            last_query: Instant::now(),
            screenshot: None,
            smoke: false,
            smoke_target: Section::Overview,
            smoke_dock: None,
            smoke_dialog: false,
            smoke_trash: false,
            smoke_about: false,
            smoke_hold_until: None,
            smoke_hold_released: false,
            smoke_about_started: None,
            smoke_final_started: None,
            smoke_about_settled: false,
            smoke_started: Instant::now(),
            smoke_sections: Vec::new(),
            smoke_final_requested: false,
            command_read: None,
            smoke_capture: false,
            smoke_management: None,
            smoke_launch_requests: Vec::new(),
            skill_catalog: Vec::new(),
            drawer: workbench::DrawerState::default(),
            inspector_preferences: BTreeMap::new(),
            list_preferences: workbench::ListPreferences::default(),
            about_open: false,
            navigation_reveal: true,
        };

        let mut fixture = Self {
            app,
            ctx,
            _dir: dir,
            path,
        };
        fixture.app.submit(
            NativeAction::Open(OpenOptions {
                path: fixture.path.clone(),
                mode: if existing { OpenMode::Existing } else { OpenMode::Create {
                    name: "App workflow fixture".into(),
                } },
                read_only: false,
            }),
            Purpose::Open(fixture.path.clone()),
        );
        fixture.until(|app| {
            app.handle.status().phase == ServicePhase::Ready && app.pending.is_empty()
        });
        fixture
    }
    fn source_area(&self)->egui::Rect {
        if self.app.section==Section::Chat {self.app.drawer.body_rect.expect("Chat source drawer must be visible")} else {self.ctx.available_rect()}
    }
    fn scroll_drawer_top(&mut self) {
        let area=self.source_area();
        self.frame(vec![egui::Event::PointerMoved(area.center()),egui::Event::MouseWheel{unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.,10000.),modifiers:Default::default()}]);
        self.frame(vec![]);
    }
    fn scroll_drawer_down(&mut self) {
        let area=self.source_area();
        self.frame(vec![egui::Event::PointerMoved(area.center()),egui::Event::MouseWheel{unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.,-area.height()*0.5),modifiers:Default::default()}]);
        self.frame(vec![]);
    }
    fn assert_drawer_texts(&mut self,needles:&[&str]) {
        self.scroll_drawer_top();
        let mut remaining=needles.to_vec();let mut observed=String::new();
        for _ in 0..60 {
            let out=self.frame(vec![]);let area=self.source_area();
            let text=visible_texts(&out,area).join("\n");observed.push_str(&text);observed.push('\n');
            remaining.retain(|needle|!text.contains(needle));
            if remaining.is_empty() {return;}
            self.scroll_drawer_down();
        }
        panic!("Actual scrolled drawer text missing {remaining:?}: {observed}");
    }
    fn new_with_config(config: impl FnOnce(&std::path::Path) -> ServiceConfig) -> Self {
        Self::configured(config)
    }
    fn click_visible(&mut self, label: &str) -> egui::FullOutput {
        let mut output = self.frame(vec![]);
        for _ in 0..24 {
            if let Some(pos) = output.shapes.iter().rev().find_map(|clipped| {
                let egui::epaint::Shape::Text(text) = &clipped.shape else { return None; };
                let rect = text.visual_bounding_rect();
                (text.galley.text() == label && clipped.clip_rect.contains_rect(rect)).then(|| rect.center())
            }) {
                self.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
                return self.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
            }
            let pos=self.source_area().center();
            output=self.frame(vec![egui::Event::PointerMoved(pos),egui::Event::MouseWheel{unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.,-self.source_area().height()*0.5),modifiers:Default::default()}]);
        }
        panic!("Actual visible button missing: {label}; {:?}",Fixture::texts(&output));
    }
    fn type_composer_and_send(&mut self,cid:&str,text:&str) {
        self.app.drawer.open=false;
        select(self,Section::Chat,Some(cid));
        let output=self.frame(vec![]);
        let pos=output.shapes.iter().find_map(|clipped| {
            let egui::epaint::Shape::Text(shape)=&clipped.shape else{return None;};
            let rect=shape.visual_bounding_rect();
            (shape.galley.text().starts_with("메시지를 입력하세요.") && clipped.clip_rect.contains_rect(rect)).then(||rect.center())
        }).expect("Actual Chat editor hint must be visible");
        self.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
        self.frame(vec![egui::Event::PointerButton{pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()},egui::Event::Text(text.into())]);
        self.click_visible("보내기 · Ctrl+Enter");
        let (expected,pinned,scope,then)=self.prepare("ui.sendConversationMessage");
        assert_eq!(expected,vec![crate::model::expected(pinned.generation.get("conversations",cid).unwrap())]);
        let params=self.app.pending.values().find_map(|p|match p {Purpose::Prepare{method,params,..} if method=="ui.sendConversationMessage"=>Some(params),_=>None}).unwrap();
        assert_eq!(params["id"],cid);assert_eq!(params["message"],text);
        assert!(matches!(then,Purpose::Command{method,scope:owner,..} if method=="ui.sendConversationMessage" && owner==scope));
        println!("APP_CONNECTED actual composer click -> original Prepare {cid}");
    }
    fn wait_conversation_terminal(&mut self,cid:&str,state:&str) {
        self.until(|app| {
            let view=app.handle.view().unwrap();
            let runs=view.generation.records("agent_runs");
            if state=="completed" {
                assert!(!runs.iter().any(|r|r.data["conversation_id"]==cid && matches!(r.data["state"].as_str(),Some("failed"|"recovery_required"))),"Actual App run failed: {runs:?}");
            }
            runs.iter().any(|r|r.data["conversation_id"]==cid && r.data["state"]==state)
                && !app.pending.values().any(|p|matches!(p,Purpose::Prepare{..}|Purpose::Command{..}))
        });
        assert!(self.app.error.is_none(),"{:?}",self.app.error);
    }
    fn assert_main_visible(&mut self,cid:&str,needle:&str) {
        self.app.drawer.open=false;
        select(self,Section::Chat,Some(cid));
        let started=Instant::now();
        // A terminal store run precedes the asynchronous history reply. Drive
        // real render/poll until the requested text actually passes its clip;
        // never hydrate the cached history or substitute a snapshot assertion.
        loop {
            let out=self.frame(vec![]);
            let visible=visible_texts(&out,egui::Rect::EVERYTHING);
            if visible.iter().any(|s|s.contains(needle)) {
                println!("APP_CONNECTED actual clipped history visible {needle}");return;
            }
            assert!(started.elapsed()<Duration::from_secs(30),"Actual clipped main text missing {needle}: {visible:?}");
            std::thread::yield_now();
        }
    }
    fn source_query(&mut self,section:Section,selected:Option<&str>,method:&str,params:Value)->egui::FullOutput {
        select(self,section,selected);
        let view=self.app.handle.view().unwrap();
        self.app.source_operations.seed_query_for_app_test(&view,section,selected,method,params);
        if section==Section::Chat {
            if !self.app.drawer.open || self.app.drawer.tab!=DrawerTab::Management {self.click_visible("추가 관리");}
            assert!(self.app.drawer.open && self.app.drawer.tab==DrawerTab::Management);
        } else {
            // Non-chat SourceOperations is the real inline disclosure. Chrome
            // correctly reserves its management drawer for Chat only.
            self.app.drawer.open=false;
            let out=self.frame(vec![]);
            if !visible_texts(&out,self.source_area()).iter().any(|s|s=="조회") {self.click_visible("추가 관리");}
        }
        self.frame(vec![]);
        self.scroll_drawer_top();
        self.click_visible("조회");
        assert!(self.app.pending.values().any(|p|matches!(p,Purpose::SourceOperation{method:actual,..} if actual==method)),"Actual Source query was not submitted");
        self.until(|app|app.source_operations.query_result_for_app_test(method,selected).is_some());
        assert!(self.app.error.is_none(),"{:?}",self.app.error);
        println!("APP_CONNECTED actual Source query/result {method}");
        self.frame(vec![])
    }
    fn source_body_detail(&mut self,cid:&str,seq:&str)->egui::FullOutput {
        let view=self.app.handle.view().unwrap();
        let blob=view.generation.get("conversation_activities",seq).unwrap().data["detail"]["blob"].is_object();
        self.source_query(Section::Chat,Some(cid),"ui.conversationMsgDetail",json!({"id":cid,"seq":seq}));
        if blob {
            self.click_visible("검증된 본문 읽기");
            // Capture the actual owner-bound ticket BEFORE another render polls.
            let (ticket,scope,token,path)=self.app.pending.iter().find_map(|(ticket,p)|match p {
                Purpose::SourceBody{scope,token,path}=>Some((ticket.clone(),scope.clone(),token.clone(),path.clone())),_=>None,
            }).expect("Blob detail must submit actual SourceBody ticket");
            assert_eq!(self.app.handle.scope().as_ref(),Some(&scope));
            assert!(self.app.source_operations.owns_token(&token));
            assert_eq!(path.first().map(String::as_str),Some("@result"));
            self.until(|app|!app.pending.contains_key(&ticket));
            assert_eq!(self.app.handle.scope().as_ref(),Some(&scope));
            let loaded=self.app.source_operations.query_result_for_app_test("ui.conversationMsgDetail",Some(cid)).unwrap();
            assert!(loaded["text"].is_string() && loaded["blob"].is_null(),"Actual Body completion failed: {loaded}");
            assert!(self.app.error.is_none(),"{:?}",self.app.error);
            println!("APP_CONNECTED SourceBody exact ticket scope/token/path completed");
        }
        self.frame(vec![])
    }
    fn reopen_same_workspace(&mut self) {
        self.reopen_acceptance(true);
    }
    fn reopen_acceptance(&mut self, read_only: bool) {
        let old=self.app.handle.scope().unwrap();
        self.app.close();
        self.until(|app|app.handle.status().phase==ServicePhase::Closed && app.pending.is_empty());
        self.app.submit(NativeAction::Open(OpenOptions{path:self.path.clone(),mode:OpenMode::Existing,read_only}),Purpose::Open(self.path.clone()));
        self.until(|app|app.handle.status().phase==ServicePhase::Ready && app.pending.is_empty());
        let view=self.app.handle.view().unwrap();
        assert_eq!(view.workspace.read_only, read_only);
        assert_ne!(view.workspace.scope.open_generation,old.open_generation);
        println!("APP_CONNECTED same workspace reopen read_only={read_only}");
    }
    fn assert_tool_result_visible(&mut self,cid:&str,label:&str,needle:&str) {
        self.app.drawer.open=false;select(self,Section::Chat,Some(cid));
        let before=self.frame(vec![]);
        if !visible_texts(&before,egui::Rect::EVERYTHING).iter().any(|s|s.contains(needle)) { self.click_visible(label); }
        let out=self.frame(vec![]);
        assert!(visible_texts(&out,egui::Rect::EVERYTHING).iter().any(|s|s.contains(needle)),"Actual opened tool result missing {needle}");
    }
    fn frame(&mut self, events: Vec<egui::Event>) -> egui::FullOutput {
        self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1600., 6000.),
                )),
                events,
                ..Default::default()
            },
            |ctx| self.app.render(ctx),
        )
    }
    fn texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) => Some(text.galley.text().to_owned()),
                _ => None,
            })
            .collect()
    }
    fn click(&mut self, label: &str) {
        let output = self.frame(vec![]);
        let pos = output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.visual_bounding_rect().center())
                }
                _ => None,
            })
            .unwrap_or_else(|| {
                panic!(
                    "Missing actual UI button {label}: {:?}",
                    Self::texts(&output)
                )
            });
        self.frame(vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
        ]);
        self.frame(vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: Default::default(),
        }]);
    }
    fn until(&mut self, condition: impl Fn(&NativeApp) -> bool) {
        let started = Instant::now();
        while !condition(&self.app) {
            assert!(
                started.elapsed() < Duration::from_secs(30),
                "Actual app pipeline timed out: {:?}",
                self.app.error
            );
            self.frame(vec![]);
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    fn setup(&mut self, method: &str, params: Value) -> Value {
        println!("NATIVE_APP_FIXTURE setup {method}");
        self.app.section = Section::Overview;
        self.app.selected.insert(Section::Overview, None);
        self.frame(vec![]);
        let scope = self.app.handle.scope().unwrap();
        self.app.command(
            method.into(),
            params.clone(),
            vec![],
            Purpose::Command {
                method: method.into(),
                params,
                section: Section::Overview,
                selected: None,
                scope,
            },
        );
        self.until(|app| {
            !app.pending
                .values()
                .any(|p| matches!(p, Purpose::Prepare { .. } | Purpose::Command { .. }))
        });
        assert!(
            self.app.error.is_none(),
            "Setup {method} failed: {:?}",
            self.app.error
        );
        self.app.panes.projection.get(method).unwrap().clone()
    }
    fn prepare(&self, method: &str) -> (&[Expected], &NativeReadView, &WorkspaceScope, &Purpose) {
        self.app
            .pending
            .values()
            .find_map(|purpose| match purpose {
                Purpose::Prepare {
                    method: actual,
                    expected,
                    pinned,
                    scope,
                    then,
                    ..
                } if actual == method => Some((expected.as_slice(), pinned, scope, then.as_ref())),
                _ => None,
            })
            .unwrap_or_else(|| panic!("Actual UI did not submit {method} Prepare"))
    }
    fn shutdown(mut self) {
        self.app.submit(NativeAction::Shutdown, Purpose::Shutdown);
        self.until(|app| app.service.is_none());
        assert!(self.app.allow_exit);
    }
}

fn configured_profile(fixture: &mut Fixture, name: &str) -> String {
    fixture.setup("ui.saveLLMProfile", json!({"p":{"name":name,"model":"fixture","format":"openai","api_key":{"secret_ref":artex_core::new_id("secret"),"hint":"isolated fixture reference"}}}))["id"].as_str().unwrap().into()
}
fn source_task(fixture: &mut Fixture, name: &str) -> String {
    let id=fixture.setup(
        "ui.createTask",
        json!({"input":{"name":name,"description":name,"goal":"Local fixture scope only","seedFirstIntent":false}}),
    )["id"].as_str().unwrap().to_owned();
    // Context tasks must be quiescent before the target editor workflow. The
    // public create facade admits running tasks even without a first intent;
    // leaving them active makes a later pause race legitimate runtime CAS.
    // Use the real stop UI and its full monitor-preview dependency versions,
    // never retry/rebase a conflicting Prepare or patch the stored task.
    select(fixture, Section::Tasks, Some(&id));
    fixture.click("작업 중지·정리");
    fixture.until(|app| {
        let view = app.handle.view().unwrap();
        view.generation
            .get("tasks", &id)
            .is_some_and(|t| t.data["paused"] == true && t.data["queued"] == false)
            && !app
                .pending
                .values()
                .any(|p| matches!(p, Purpose::NativeOperation { .. }))
    });
    let view = fixture.app.handle.view().unwrap();
    let runtime = view
        .generation
        .records("task_runtime")
        .into_iter()
        .find(|r| r.data["task_id"] == id)
        .unwrap();
    assert!(runtime.data["last_active_clock"].is_null());
    assert!(
        fixture.app.error.is_none(),
        "Source stop failed: {:?}",
        fixture.app.error
    );
    println!("NATIVE_APP_FIXTURE source stopped and clock retired: {name}");
    id
}
fn select(fixture: &mut Fixture, section: Section, id: Option<&str>) {
    fixture.app.section = section;
    fixture.app.selected.insert(section, id.map(str::to_owned));
    fixture.frame(vec![]);
}
fn assert_references(fixture: &Fixture, task: &str, profile: &str, source: &str) {
    let view = fixture.app.handle.view().unwrap();
    assert_eq!(
        view.generation.get("tasks", task).unwrap().data["llm_profile_id"],
        profile
    );
    let profiles = view
        .generation
        .records("task_llm_profiles")
        .into_iter()
        .filter(|r| r.data["task_id"] == task)
        .collect::<Vec<_>>();
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].data["profile_id"], profile);
    assert_eq!(profiles[0].data["position"], 0);
    let sources = view
        .generation
        .records("task_relations")
        .into_iter()
        .filter(|r| r.data["task_id"] == task)
        .collect::<Vec<_>>();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].data["source_task_id"], source);
}
#[test]
fn actual_app_profile_delete_unavailable_selection_create_and_archive_restore_reopen_display() {
    let mut fixture = Fixture::new();
    let keep_profile = configured_profile(&mut fixture, "Keep fixture profile");
    let drop_profile = configured_profile(&mut fixture, "Remove fixture profile");
    let keep_source = source_task(&mut fixture, "Keep fixture source");
    let drop_source = source_task(&mut fixture, "Archived fixture source");
    select(&mut fixture, Section::Tasks, None);
    // Description/goal are fixture context. Target input is actual checkbox
    // selection, explicit stale-reference removal, and the real create click.
    fixture.app.panes.title = "UI-selected final task".into();
    fixture.app.panes.description =
        "Creation draft survives profile deletion and source archive".into();
    fixture.app.panes.goal = "Local fixture scope only".into();
    fixture.click("Keep fixture profile");
    fixture.click("Remove fixture profile");
    fixture.click("정보를 공유할 직접 원본 작업");
    fixture.click("Keep fixture source");
    fixture.click("Archived fixture source");
    assert_eq!(
        fixture.app.panes.selected_profiles,
        vec![keep_profile.clone(), drop_profile.clone()]
    );
    assert_eq!(
        fixture.app.panes.selected_sources,
        vec![keep_source.clone(), drop_source.clone()]
    );

    select(&mut fixture, Section::Models, Some(&drop_profile));
    fixture.click("이 항목을 삭제하는 것을 확인합니다.");
    let profile_before = fixture
        .app
        .handle
        .view()
        .unwrap()
        .generation
        .get("llm_profiles", &drop_profile)
        .unwrap()
        .clone();
    fixture.click("삭제");
    let (expected, pinned, scope, then) = fixture.prepare("ui.deleteLLMProfile");
    assert!(expected.contains(&crate::model::expected(&profile_before)));
    assert_eq!(
        pinned
            .generation
            .get("llm_profiles", &drop_profile)
            .unwrap()
            .header
            .version,
        profile_before.header.version
    );
    assert_eq!(Some(scope.clone()), fixture.app.handle.scope());
    assert!(matches!(then,Purpose::FormsSave{token:Some(_),scope:owner} if owner==scope));
    fixture.until(|app| {
        app.handle
            .view()
            .unwrap()
            .generation
            .get("llm_profiles", &drop_profile)
            .is_none()
            && !app
                .pending
                .values()
                .any(|p| matches!(p, Purpose::Prepare { .. } | Purpose::FormsSave { .. }))
    });
    assert!(fixture.app.error.is_none(), "{:?}", fixture.app.error);
    assert!(
        fixture
            .app
            .notices
            .iter()
            .any(|n| n == "설정을 저장했습니다.")
    );

    // Archive the selected source through its real task action, retaining the
    // creation draft and its now-unavailable selected identity until removal.
    let view = fixture.app.handle.view().unwrap();
    let source = view.generation.get("tasks", &drop_source).unwrap();
    assert_eq!(source.data["paused"], true);
    assert_eq!(source.data["queued"], false);
    select(&mut fixture, Section::Tasks, Some(&drop_source));
    fixture.click("보관");
    let (expected, pinned, scope, _) = fixture.prepare("ui.archiveTask");
    assert!(expected.contains(&crate::model::expected(
        pinned.generation.get("tasks", &drop_source).unwrap()
    )));
    assert_eq!(Some(scope.clone()), fixture.app.handle.scope());
    fixture.until(|app| {
        app.handle
            .view()
            .unwrap()
            .generation
            .records("task_archives")
            .iter()
            .any(|a| a.data["task_id"] == drop_source && a.data["state"] == "ready")
    });
    let archive = fixture
        .app
        .handle
        .view()
        .unwrap()
        .generation
        .records("task_archives")
        .into_iter()
        .find(|a| a.data["task_id"] == drop_source)
        .unwrap()
        .header
        .id
        .clone();
    select(&mut fixture, Section::Tasks, None);
    assert_eq!(fixture.app.panes.title, "UI-selected final task");
    assert_eq!(
        fixture.app.panes.description,
        "Creation draft survives profile deletion and source archive"
    );
    assert_eq!(
        fixture.app.panes.selected_profiles,
        vec![keep_profile.clone(), drop_profile.clone()]
    );
    assert_eq!(
        fixture.app.panes.selected_sources,
        vec![keep_source.clone(), drop_source.clone()]
    );
    fixture.click(&format!("모델 선택 해제: {drop_profile}"));
    fixture.click(&format!("원본 작업 선택 해제: {drop_source}"));
    assert_eq!(
        fixture.app.panes.selected_profiles,
        vec![keep_profile.clone()]
    );
    assert_eq!(
        fixture.app.panes.selected_sources,
        vec![keep_source.clone()]
    );
    fixture.click("작업 만들기");
    let (expected, pinned, scope, then) = fixture.prepare("ui.createTask");
    assert!(
        expected.is_empty(),
        "Panes emits no invented CAS; actual Prepare fills referenced records from this pinned read"
    );
    assert!(
        pinned
            .generation
            .get("llm_profiles", &keep_profile)
            .is_some()
    );
    assert!(
        matches!(then,Purpose::Command{method,scope:owner,..} if method=="ui.createTask" && owner==scope)
    );
    fixture.until(|app| {
        app.handle
            .view()
            .unwrap()
            .generation
            .records("tasks")
            .iter()
            .any(|r| r.data["name"] == "UI-selected final task")
            && !app.pending.values().any(|purpose| {
                matches!(purpose,
                Purpose::Prepare { method, .. } | Purpose::Command { method, .. }
                    if method == "ui.createTask")
            })
    });
    let created = fixture
        .app
        .handle
        .view()
        .unwrap()
        .generation
        .records("tasks")
        .into_iter()
        .find(|r| r.data["name"] == "UI-selected final task")
        .unwrap()
        .header
        .id
        .clone();
    assert_eq!(fixture.app.section, Section::Tasks);
    assert_eq!(
        fixture
            .app
            .selected
            .get(&Section::Tasks)
            .and_then(Option::as_deref),
        Some(created.as_str())
    );
    assert_references(&fixture, &created, &keep_profile, &keep_source);
    select(&mut fixture, Section::Tasks, Some(&created));
    let output = fixture.frame(vec![]);
    let visible = Fixture::texts(&output).join("\n");
    assert!(visible.contains("UI-selected final task"));
    assert!(visible.contains("Creation draft survives profile deletion and source archive"));

    select(&mut fixture, Section::Archives, Some(&archive));
    let archived = fixture
        .app
        .handle
        .view()
        .unwrap()
        .generation
        .get("task_archives", &archive)
        .unwrap()
        .clone();
    fixture.click("보관 작업 복구");
    let (expected, pinned, scope, then) = fixture.prepare("ui.restoreTaskArchive");
    assert_eq!(expected, vec![crate::model::expected(&archived)]);
    assert_eq!(
        pinned
            .generation
            .get("task_archives", &archive)
            .unwrap()
            .header
            .version,
        archived.header.version
    );
    assert!(
        matches!(then,Purpose::Command{method,scope:owner,..} if method=="ui.restoreTaskArchive" && owner==scope)
    );
    fixture.until(|app| {
        let view = app.handle.view().unwrap();
        view.generation.get("task_archives", &archive).is_none()
            && view
                .generation
                .get("tasks", &drop_source)
                .is_some_and(|t| t.data["archived_at"].is_null())
    });
    assert!(fixture.app.error.is_none(), "{:?}", fixture.app.error);
    let old_scope = fixture.app.handle.scope().unwrap();
    fixture.app.close();
    fixture
        .until(|app| app.handle.status().phase == ServicePhase::Closed && app.pending.is_empty());
    fixture.app.submit(
        NativeAction::Open(OpenOptions {
            path: fixture.path.clone(),
            mode: OpenMode::Existing,
            read_only: false,
        }),
        Purpose::Open(fixture.path.clone()),
    );
    fixture.until(|app| app.handle.status().phase == ServicePhase::Ready && app.pending.is_empty());
    assert_ne!(
        fixture.app.handle.scope().unwrap().open_generation,
        old_scope.open_generation
    );
    assert!(
        fixture
            .app
            .handle
            .view()
            .unwrap()
            .generation
            .get("llm_profiles", &drop_profile)
            .is_none()
    );
    assert!(
        fixture
            .app
            .handle
            .view()
            .unwrap()
            .generation
            .get("task_archives", &archive)
            .is_none()
    );
    assert_references(&fixture, &created, &keep_profile, &keep_source);
    select(&mut fixture, Section::Tasks, Some(&drop_source));
    let visible = Fixture::texts(&fixture.frame(vec![])).join("\n");
    assert!(visible.contains("Archived fixture source"));
    assert!(visible.contains("Local fixture scope only"));
    select(&mut fixture, Section::Models, None);
    let visible = Fixture::texts(&fixture.frame(vec![])).join("\n");
    assert!(visible.contains("Keep fixture profile"));
    assert!(!visible.contains("Remove fixture profile"));
    select(&mut fixture, Section::Tasks, Some(&created));
    let visible = Fixture::texts(&fixture.frame(vec![])).join("\n");
    assert!(visible.contains("UI-selected final task"));
    assert!(visible.contains("Creation draft survives profile deletion and source archive"));
    println!(
        "NATIVE_APP_E2E profile delete + explicit stale selections + create + archive restore + same-workspace reopen + actual display passed"
    );
    fixture.shutdown();
}
#[test]
fn actual_app_form_conflict_lock_and_post_click_prepare_keep_original_cas() {
    let mut fixture = Fixture::new();
    let profile = configured_profile(&mut fixture, "Original profile draft");
    select(&mut fixture, Section::Models, Some(&profile));
    fixture.click("이 항목을 삭제하는 것을 확인합니다.");
    fixture.click("삭제");
    let (expected, pinned, scope, then) = fixture.prepare("ui.deleteLLMProfile");
    let original = pinned
        .generation
        .get("llm_profiles", &profile)
        .unwrap()
        .clone();
    let original_expected = crate::model::expected(&original);
    assert!(expected.contains(&original_expected));
    assert!(matches!(then,Purpose::FormsSave{token:Some(_),scope:owner} if owner==scope));
    let scope = scope.clone();
    // The real click already submitted Prepare. A concurrent, valid command
    // commits before the next app.poll. Never consume the app's completions in
    // the fixture: only the production poll may merge and dispatch deletion.
    let command = NativeCommand::new(
        "ui.saveLLMProfile",
        json!({"p":{"id":profile,"name":"Concurrent profile update","model":"fixture","format":"openai","api_key":original.data["api_key"]}}),
        vec![original_expected],
    );
    let _concurrent = fixture
        .app
        .handle
        .try_submit(NativeAction::Command { scope, command })
        .unwrap();
    let started = Instant::now();
    while fixture
        .app
        .handle
        .view()
        .unwrap()
        .generation
        .get("llm_profiles", &profile)
        .unwrap()
        .header
        .version
        == original.header.version
    {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "Concurrent valid fixture command did not commit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    fixture.until(|app| {
        !app.pending
            .values()
            .any(|p| matches!(p, Purpose::Prepare { .. } | Purpose::FormsSave { .. }))
    });
    let output = fixture.frame(vec![]);
    let visible = Fixture::texts(&output).join("\n");
    assert!(
        visible.contains("VERSION_CONFLICT"),
        "Actual app did not display original-version rejection: {visible}"
    );
    assert!(
        visible.contains("Original profile draft"),
        "Editor draft must survive rejected deletion: {visible}"
    );
    assert_eq!(
        fixture
            .app
            .handle
            .view()
            .unwrap()
            .generation
            .get("llm_profiles", &profile)
            .unwrap()
            .data["name"],
        "Concurrent profile update"
    );
    assert!(
        !fixture
            .app
            .pending
            .values()
            .any(|p| matches!(p,Purpose::Prepare{method,..} if method=="ui.deleteLLMProfile"))
    );
    // A rendered stale editor locks the deletion button before admission too.
    fixture.click("삭제");
    assert!(
        !fixture
            .app
            .pending
            .values()
            .any(|p| matches!(p,Purpose::Prepare{method,..} if method=="ui.deleteLLMProfile"))
    );
    assert!(
        fixture
            .app
            .handle
            .view()
            .unwrap()
            .generation
            .get("llm_profiles", &profile)
            .is_some()
    );
    println!(
        "NATIVE_APP_E2E stale form lock + post-click Prepare original CAS rejection + draft preserved passed"
    );
    fixture.shutdown();
}

// Whole galley text is not evidence of visibility: preserve row/glyph clips,
// and optionally constrain the check to the actual drawer viewport.
fn visible_texts(output:&egui::FullOutput,area:egui::Rect)->Vec<String> {
    fn collect(shape:&egui::epaint::Shape,clip:egui::Rect,out:&mut Vec<String>) {
        match shape {
            egui::epaint::Shape::Vec(items)=>for item in items {collect(item,clip,out)},
            egui::epaint::Shape::Text(text) if text.angle==0. && clip.intersects(text.visual_bounding_rect())=> {
                let mut visible=String::new();let mut has_visible_glyph=false;
                for row in &text.galley.rows {
                    let offset=text.pos.to_vec2()+row.pos.to_vec2();
                    // Font logical line boxes include invisible descent/leading.
                    // Fully clipped-in painted ink proves this row's glyphs,
                    // never the entire (possibly clipped/elided) galley text.
                    let ink=row.visuals.mesh_bounds.translate(offset);
                    if ink.is_positive() && clip.contains_rect(ink) {
                        visible.push_str(&row.text());has_visible_glyph=true;
                    } else {
                        for glyph in &row.glyphs {
                            if clip.contains_rect(glyph.logical_rect().translate(offset)) {
                                visible.push(glyph.chr);has_visible_glyph=true;
                            } else {visible.push('\u{fffd}');}
                        }
                    }
                    // Soft wrapping does not insert source characters. Keep
                    // hidden glyph placeholders and real hard newlines so a
                    // marker cannot bridge clipped text or unrelated shapes.
                    if row.ends_with_newline {visible.push('\n');}
                }
                if has_visible_glyph {out.push(visible);}
            }
            _=>{}
        }
    }
    let mut out=Vec::new();for shape in &output.shapes {collect(&shape.shape,shape.clip_rect.intersect(area),&mut out)}out
}
#[path="app_e2e_transport.rs"]
mod transport;
#[path="app_host_e2e_tests.rs"]
mod host;

#[path="app_provider_e2e_tests.rs"]
mod provider;
#[path="app_feedback_tests.rs"]
mod feedback;
#[path="app_site_report_e2e_tests.rs"]
mod site_report;
#[path="app_model_migration_tests.rs"]
mod model_migration;
#[path="app_stable_core_e2e_tests.rs"]
mod stable_core;
#[path="app_acceptance_helpers.rs"]
mod acceptance_helpers;
#[path="app_acceptance_domain_tests.rs"]
mod acceptance_domain;
#[path="app_acceptance_config_tests.rs"]
mod acceptance_config;
#[path="app_acceptance_tools_tests.rs"]
mod acceptance_tools;
#[path="app_acceptance_general_tests.rs"]
mod acceptance_general;
