//! Pause intent is pinned to user state; engine-owned clock drift is read by the
//! single writer. Only this admission records its derived dependency namespace.
use super::*;
use artex_core::{Change,Record};
const CLOCK_ADMISSION_KEY:&str="$artex_writer_clock_v1";

impl Router {
    pub(super) fn task_pause_clock_admission(&self,r:&DispatchRequest,b:&CommandBatch)->Result<BTreeMap<String,String>> {
        let mut clocks=BTreeMap::new();
        if !self.native_mode||r.method!="ui.controlTasksBatch"||r.params["action"]!="pause" {return Ok(clocks);}
        let snapshot=self.snapshot()?;
        let dependencies=b.expected.iter().filter(|e|e.collection=="task_runtime").map(|e|e.id.as_str()).collect::<HashSet<_>>();
        // A clock exception cannot hide missing dependencies or a supplied
        // unrelated runtime, even though the user can inspect other tasks.
        let supplied=r.expected.iter().filter(|e|e.collection=="task_runtime").map(|e|e.id.as_str()).collect::<HashSet<_>>();
        if supplied!=dependencies {return Err(Error::new("VERSION_CONFLICT","Pause requires the exact task runtime identity set"));}
        let targets=r.params["taskIds"].as_array().ok_or_else(||Error::new("INVALID_PARAMS","Task IDs required"))?.iter().filter_map(Value::as_str).collect::<HashSet<_>>();
        for change in &b.changes {
            let Change::Put{collection,id,data}=change else {continue;};
            if collection!="task_runtime" {continue;}
            let current=snapshot.get(collection,id).ok_or_else(||Error::new("VERSION_CONFLICT","Pause runtime disappeared"))?;
            let Some(task)=current.data["task_id"].as_str().and_then(|id|snapshot.get("tasks",id)) else {continue;};
            if !targets.contains(task.header.id.as_str())||task.data["status"]!="running"||task.data["paused"]==true
                ||!task.data["deleted_at"].is_null()||!task.data["archived_at"].is_null()||current.data["accounting_state"]!="known" {continue;}
            let mut cleared=current.data.clone();cleared["last_active_clock"]=Value::Null;
            if &cleared!=data {continue;}
            let supplied=r.expected.iter().find(|e|e.collection==*collection&&e.id==*id).unwrap();
            // Older bookkeeping is eligible; a claimed future version is not
            // an observed tick and keeps ordinary CAS rejection semantics.
            let observed=parse_u64(&supplied.version)?;
            if observed>0&&observed<parse_u64(&current.header.version)? {
                clocks.insert(id.clone(),current.header.version.clone());
            }
        }
        Ok(clocks)
    }
    pub(super) fn task_pause_clock_params(params:&Value,clocks:&BTreeMap<String,String>)->Value {
        let mut out=params.clone();if !clocks.is_empty() {out[CLOCK_ADMISSION_KEY]=json!(clocks.keys().collect::<Vec<_>>());}out
    }
    pub(super) fn task_clock_receipt_matches(old:&Record,r:&DispatchRequest)->Result<Option<bool>> {
        let mut params=old.data["normalized_params"].clone();
        let Some(marker)=params.as_object_mut().and_then(|p|p.remove(CLOCK_ADMISSION_KEY)) else {return Ok(None);};
        let corrupt=||Error::new("CORRUPT_STORE","Malformed writer-owned task clock admission");
        if old.data["method"]!="ui.controlTasksBatch"||params["action"]!="pause" {return Err(corrupt());}
        let clocks:Vec<String>=serde_json::from_value(marker).map_err(|_|corrupt())?;
        if clocks.is_empty()||clocks.windows(2).any(|pair|pair[0]>=pair[1])||clocks.iter().any(|id|!artex_core::valid_id(id,"taskruntime")) {return Err(corrupt());}
        let expected:Vec<Expected>=serde_json::from_value(old.data["expected"].clone()).map_err(|_|corrupt())?;
        let stored=expected.iter().map(|e|((e.collection.as_str(),e.id.as_str()),e.version.as_str())).collect::<BTreeMap<_,_>>();
        if stored.len()!=expected.len()||clocks.iter().any(|id|!stored.get(&("task_runtime",id.as_str())).is_some_and(|version|parse_u64(version).is_ok_and(|version|version>0))) {return Err(corrupt());}
        // Expected order and identity remain part of the durable command. Only
        // versions explicitly classified at the original admission may differ.
        let same=expected.len()==r.expected.len()&&expected.iter().zip(&r.expected).all(|(before,after)|before.collection==after.collection&&before.id==after.id&&(before.version==after.version||(before.collection=="task_runtime"&&clocks.binary_search(&before.id).is_ok()&&parse_u64(&after.version).ok().zip(parse_u64(&before.version).ok()).is_some_and(|(observed,writer)|observed>0&&observed<=writer))));
        Ok(Some(params==r.params&&same))
    }
}

#[cfg(test)]
#[path="task_clock_admission_tests.rs"]
mod tests;
