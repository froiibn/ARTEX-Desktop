//! Shared connected-acceptance drivers. Only initial user inputs are fixtures;
//! requests, CAS, execution, persistence, callbacks and rendered results are real.
use super::*;

impl Fixture {
    pub(super) fn source_action(&mut self, section:Section, selected:Option<&str>, method:&str, params:Value)->Value {
        let action=crate::forms::operations::catalog().iter()
            .find(|a| a.section==section && a.method==method).expect("Real curated action").clone();
        select(self,section,selected);
        self.until(|app|app.pending.is_empty());
        let view=self.app.handle.view().unwrap();
        self.app.source_operations.seed_action_for_app_test(&view,section,selected,method,params);
        if section==Section::Chat {
            if !self.app.drawer.open || self.app.drawer.tab!=DrawerTab::Management {self.click_visible("추가 관리");}
        } else {
            self.app.drawer.open=false;
            let out=self.frame(vec![]);
            let button=if action.mutation {"변경 실행"} else {"조회"};
            if !visible_texts(&out,self.source_area()).iter().any(|s|s==button) {self.click_visible("추가 관리");}
        }
        self.frame(vec![]);
        if action.destructive { self.click_visible("선택한 대상의 삭제·연결 해제·초기화를 확인했습니다."); }
        self.click_visible(if action.mutation {"변경 실행"} else {"조회"});
        if action.mutation {
            let (_,pinned,scope,then)=self.prepare(method);
            assert_eq!(pinned.workspace.scope,*scope);
            assert!(matches!(then,Purpose::SourceOperation{method:actual,scope:owner,..} if actual==method && owner==scope));
        } else {
            assert!(self.app.pending.values().any(|p|matches!(p,Purpose::SourceOperation{method:actual,..} if actual==method)));
        }
        self.until(|app|app.source_operations.action_result_for_app_test(method,selected).is_some());
        assert!(self.app.error.is_none(),"{method}: {:?}",self.app.error);
        let result=self.app.source_operations.action_result_for_app_test(method,selected).unwrap();
        println!("ACCEPT_SOURCE {method} {}",serde_json::to_string(&result).unwrap());
        result
    }

    pub(super) fn source_result_visible(&mut self, method:&str, selected:Option<&str>, needle:&str) {
        let result=self.app.source_operations.action_result_for_app_test(method,selected).expect("Real completed result");
        let mut target=vec!["@result".into()];
        target.extend(result_path(&result,needle).unwrap_or_else(||panic!("Result does not contain {needle}: {result}")));
        self.observe_source_result_path(method,selected,target,needle);
    }

    pub(super) fn source_result_at_path_visible(&mut self,method:&str,selected:Option<&str>,path:&[&str],needle:&str) {
        let result=self.app.source_operations.action_result_for_app_test(method,selected).expect("Real completed result");
        let relative=if path.first()==Some(&"@result") {&path[1..]} else {path};
        let mut value=&result;
        for segment in relative {
            value=if value.is_array() {&value[segment.parse::<usize>().expect("Actual result array index")]} else {&value[*segment]};
        }
        assert!(result_path(value,needle).is_some(),"Actual result path {path:?} missing {needle}: {value}");
        let mut target=vec!["@result".into()];target.extend(relative.iter().map(|v|(*v).to_owned()));
        // Repeated booleans/counts are meaningful only at their declared field,
        // never at an unrelated earlier leaf carrying the same literal value.
        self.observe_source_result_path(method,selected,target,needle);
    }

    fn observe_source_result_path(&mut self, method:&str, selected:Option<&str>, target:Vec<String>, needle:&str) {
        // Observe the actual CollapsingHeader response, then click its real
        // rectangle. The asserted paint is restricted to the target leaf body,
        // never an input or another field holding an identical marker.
        for _ in 0..128 {
            let out=self.frame(vec![]);
            assert!(self.app.source_operations.action_result_for_app_test(method,selected).is_some(),"Result owner changed during observation");
            let (bounds,headers)=crate::forms::operations::SourceOperations::result_widgets_for_app_test();
            let leaf=headers.iter().find(|h|h.path==target && h.open);
            // Galley glyphs may extend beyond the label's layout rect. Bound
            // the leaf by its real header and the next rendered header instead
            // of clipping descenders to the layout box (which loses letters).
            let paint=if target.len()==1 {bounds} else {leaf.and_then(|h|h.body.map(|body| {
                let bottom=headers.iter().filter(|other|other.rect.top()>=body.bottom() && other.path!=h.path)
                    .map(|other|other.rect.top()).min_by(|a,b|a.total_cmp(b))
                    .or_else(||bounds.map(|b|b.bottom())).unwrap_or(body.bottom());
                egui::Rect::from_min_max(egui::pos2(h.clip.left(),h.rect.bottom()),egui::pos2(h.clip.right(),bottom))
            }))};
            if let Some(paint)=paint {
                if visible_texts(&out,paint.intersect(self.source_area())).iter().any(|s|s.contains(needle)) {
                    println!("ACCEPT_VISIBLE {method} {target:?} {needle}");return;
                }
            }
            let closed=headers.iter().filter(|h|!h.open && target.starts_with(&h.path)).min_by_key(|h|h.path.len());
            let observation=closed.or(leaf).or_else(||headers.iter().filter(|h|target.starts_with(&h.path)).max_by_key(|h|h.path.len()));
            if let Some(h)=observation {
                if !h.open && h.clip.intersects(h.rect) {
                    let pos=h.rect.intersect(h.clip).center();
                    self.frame(vec![egui::Event::PointerMoved(pos),egui::Event::PointerButton {pos,button:egui::PointerButton::Primary,pressed:true,modifiers:Default::default()}]);
                    self.frame(vec![egui::Event::PointerButton {pos,button:egui::PointerButton::Primary,pressed:false,modifiers:Default::default()}]);
                } else {
                    let up=h.rect.top()<h.clip.top();let delta=if up {h.clip.height()*0.6} else {-h.clip.height()*0.6};
                    self.frame(vec![egui::Event::PointerMoved(h.clip.center()),egui::Event::MouseWheel {unit:egui::MouseWheelUnit::Point,delta:egui::vec2(0.,delta),modifiers:Default::default()}]);
                }
            } else {
                self.scroll_drawer_down();
            }
        }
        let out=self.frame(vec![]);
        let widgets=crate::forms::operations::SourceOperations::result_widgets_for_app_test();
        panic!("Actual target result leaf never painted {method} {target:?}: {needle}; area={:?} content={:?} widgets={widgets:?} paint={:?}",
            self.source_area(),self.ctx.content_rect(),visible_texts(&out,self.ctx.content_rect()));
    }
}

fn result_path(value:&Value,needle:&str)->Option<Vec<String>> {
    if value.get("text").is_some() && value.get("blob").is_some() {
        return value["text"].as_str().filter(|s|s.contains(needle)).map(|_|vec![]);
    }
    match value {
        Value::Object(values)=>values.iter().find_map(|(key,value)| {
            let mut path=result_path(value,needle)?;
            path.insert(0,key.clone());Some(path)
        }),
        Value::Array(values)=>values.iter().enumerate().take(100).find_map(|(index,value)| {
            let mut path=result_path(value,needle)?;path.insert(0,index.to_string());Some(path)
        }),
        Value::String(s)=>s.contains(needle).then(Vec::new),
        v=>v.to_string().contains(needle).then(Vec::new),
    }
}
