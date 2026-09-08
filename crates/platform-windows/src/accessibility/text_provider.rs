// SPDX-License-Identifier: MPL-2.0
//! Application editor patterns over immutable, nonblocking bounded text windows.
//! COM ranges retain offsets, not document buffers. Foreign COM ranges are checked
//! through windows-rs' safe object query; no unchecked `as_impl` conversion.
#![allow(non_snake_case, non_upper_case_globals)]
use super::Shared;
use bareline_platform::accessibility::*;
use std::sync::{Arc, Mutex, RwLock, Weak};
use unicode_segmentation::UnicodeSegmentation;
use windows::{core::*, Win32::{Foundation::*, System::{Com::SAFEARRAY, Ole::{SafeArrayCreateVector, SafeArrayPutElement, SafeArrayDestroy}, Variant::*}, UI::Accessibility::*}};

const LIMIT: usize = 64 * 1024;
fn unavailable() -> Error { Error::from_hresult(windows::core::HRESULT(UIA_E_ELEMENTNOTAVAILABLE as i32)) }
fn invalid() -> Error { Error::from_hresult(E_INVALIDARG) }
fn pending() -> Error { Error::from_hresult(HRESULT(0x8000000Au32 as i32)) }
fn unsupported() -> Error { Error::from_hresult(windows::core::HRESULT(UIA_E_NOTSUPPORTED as i32)) }
fn array(values: &[IUnknown]) -> Result<*mut SAFEARRAY> {
    // SAFEARRAY owns an AddRef for every inserted interface. On failure destroy
    // the partial array, releasing those references before returning the error.
    let sa = unsafe { SafeArrayCreateVector(VT_UNKNOWN, 0, values.len() as u32) };
    if sa.is_null() { return Err(Error::from_hresult(E_OUTOFMEMORY)); }
    for (index, value) in values.iter().enumerate() {
        let index = index as i32;
        if let Err(error) = unsafe { SafeArrayPutElement(sa, &index, value.as_raw()) } {
            let _ = unsafe { SafeArrayDestroy(sa) };
            return Err(error);
        }
    }
    Ok(sa)
}
fn rectangles(values: &[f64]) -> Result<*mut SAFEARRAY> {
    let sa = unsafe { SafeArrayCreateVector(VT_R8, 0, values.len() as u32) };
    if sa.is_null() { return Err(Error::from_hresult(E_OUTOFMEMORY)); }
    for (index, value) in values.iter().enumerate() {
        if let Err(error) = unsafe { SafeArrayPutElement(sa, &(index as i32), (value as *const f64).cast()) } {
            let _ = unsafe { SafeArrayDestroy(sa) }; return Err(error);
        }
    }
    Ok(sa)
}
pub(super) struct Factory { life: Arc<Life> }
struct Life {
    source: RwLock<Option<Arc<dyn AccessibilityTextSource>>>,
    shared: Arc<Mutex<Shared>>,
    notify: Arc<dyn Fn() + Send + Sync>,
}
impl Factory {
    pub(super) fn new(shared: Arc<Mutex<Shared>>, notify: Arc<dyn Fn() + Send + Sync>) -> Self {
        Self { life: Arc::new(Life { source: RwLock::new(None), shared, notify }) }
    }
    pub(super) fn set_source(&self, source: Option<Arc<dyn AccessibilityTextSource>>) {
        let mut current = self.life.source.write().unwrap_or_else(|e| e.into_inner());
        // Retain the bounded page cache across layouts of the same revision.
        if current.as_ref().map(|v| v.identity()) != source.as_ref().map(|v| v.identity()) { *current = source; }
    }
}
impl accesskit_windows::PatternOverride for Factory {
    fn property(&self, tree: accesskit::TreeId, node: accesskit::NodeId, property: UIA_PROPERTY_ID) -> Option<Result<VARIANT>> {
        if tree == accesskit::TreeId::ROOT && node.0 == 2 && matches!(property, UIA_IsTextPatternAvailablePropertyId | UIA_IsTextPattern2AvailablePropertyId | UIA_IsTextEditPatternAvailablePropertyId) && self.life.source.read().unwrap_or_else(|e| e.into_inner()).is_some() {
            Some(Ok(true.into()))
        } else { None }
    }
    fn pattern(&self, tree: accesskit::TreeId, node: accesskit::NodeId, pattern: UIA_PATTERN_ID, enclosing: IRawElementProviderSimple) -> Option<Result<IUnknown>> {
        if tree != accesskit::TreeId::ROOT || node.0 != 2 || !matches!(pattern, UIA_TextPatternId | UIA_TextPattern2Id | UIA_TextEditPatternId) { return None; }
        // No source means the ordinary AccessKit viewport provider remains valid.
        if self.life.source.read().unwrap_or_else(|e| e.into_inner()).is_none() { return None; }
        let provider: ITextEditProvider = Provider { life: Arc::downgrade(&self.life), enclosing }.into();
        Some(match pattern {
            UIA_TextPatternId => provider.cast::<ITextProvider>().and_then(|v| v.cast()),
            UIA_TextPattern2Id => provider.cast::<ITextProvider2>().and_then(|v| v.cast()),
            _ => provider.cast(),
        })
    }
}
#[derive(Clone, PartialEq, Eq)]
struct Overlay { start: usize, end: usize, text: String }
struct View {
    source: Arc<dyn AccessibilityTextSource>,
    overlay: Option<Overlay>,
    selection: (usize, usize),
    visible: (usize, usize),
    visible_ranges: Vec<(usize, usize)>,
    page: usize,
}
impl Life {
    fn geometry(&self, view: &View, enclosing: &IRawElementProviderSimple) -> Result<Vec<AccessibilityTextBox>> {
        let (mut boxes, origin) = {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if shared.snapshot.text_context.as_ref().map(|c| c.source_identity) != Some(view.source.identity()) { return Err(unavailable()); }
            let node = shared.snapshot.nodes.iter().find(|n| n.id == 2).ok_or_else(unavailable)?;
            (shared.snapshot.text_geometry.clone(), (node.bounds[0], node.bounds[1]))
        };
        let fragment: IRawElementProviderFragment = enclosing.cast()?;
        let screen = unsafe { fragment.BoundingRectangle()? };
        for rect in &mut boxes {
            rect.bounds[0] += screen.left-origin.0;
            rect.bounds[1] += screen.top-origin.1;
        }
        Ok(boxes)
    }
    fn view(&self) -> Result<View> {
        let source = self.source.read().unwrap_or_else(|e| e.into_inner()).clone().ok_or_else(unavailable)?;
        let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        let context = shared.snapshot.text_context.as_ref().ok_or_else(unavailable)?;
        if shared.snapshot.nodes.iter().any(|node| node.id == 2 && node.disabled) { return Err(unavailable()); }
        // Source and tree publication can straddle a UIA call. A mixed pair is
        // unavailable, never a new document with the old document's selection.
        if context.source_identity != source.identity() { return Err(unavailable()); }
        let (a, c) = context.selection;
        if a > source.len() || c > source.len() { return Err(unavailable()); }
        // The renderer displays preedit inserted at the caret. Selection is only
        // replaced on commit; describe the actual current virtual text domain.
        let overlay = context.composition.as_ref().map(|text| Overlay { start: c, end: c, text: text.clone() });
        let visible = shared.snapshot.text.as_ref().map(|t| (t.start_byte, t.start_byte + t.value.len())).unwrap_or((c,c));
        // Geometry is already in the source-mapped virtual byte domain. Capture
        // it with selection/composition under this same coherent publication.
        // A folded header and footer are distinct visible ranges even though
        // the bounded text cache contains only the header's contiguous window.
        if shared.snapshot.text_geometry.len() > 4096 { return Err(unavailable()); }
        let length=source.len().checked_add(overlay.as_ref().map_or(0,|o|o.text.len())).ok_or_else(unavailable)?;
        let mut spans=Vec::with_capacity(shared.snapshot.text_geometry.len());
        for rect in &shared.snapshot.text_geometry {
            if rect.start>rect.end || rect.end>length {return Err(unavailable());}
            if rect.start<rect.end {spans.push((rect.start,rect.end));}
        }
        spans.sort_unstable();
        let mut visible_ranges:Vec<(usize,usize)>=Vec::new();
        for (start,end) in spans {
            if let Some(previous)=visible_ranges.last_mut() && start<=previous.1 {
                previous.1=previous.1.max(end);
            } else {visible_ranges.push((start,end));}
        }
        Ok(View { source, overlay, selection: (a,c), visible, visible_ranges, page: visible.1.saturating_sub(visible.0).clamp(1024, LIMIT) })
    }
    fn action(&self, action: AccessibilityAction) -> Result<()> {
        let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
        if shared.actions.len() >= 256 { return Err(pending()); }
        shared.actions.push(action);
        drop(shared);
        (self.notify)();
        Ok(())
    }
}
impl View {
    fn len(&self) -> usize { self.overlay.as_ref().map_or(self.source.len(), |o| self.source.len() - (o.end-o.start) + o.text.len()) }
    fn virtual_offset(&self, offset: usize) -> usize {
        self.overlay.as_ref().map_or(offset, |o| {
            if offset <= o.start { offset } else if offset < o.end { o.start } else { offset - (o.end-o.start) + o.text.len() }
        })
    }
    fn committed(&self, offset: usize) -> Result<usize> {
        if offset > self.len() { return Err(invalid()); }
        match &self.overlay {
            Some(o) if offset > o.start && offset < o.start + o.text.len() => Err(unsupported()),
            Some(o) if offset >= o.start + o.text.len() => Ok(offset - o.text.len() + (o.end-o.start)),
            _ => Ok(offset),
        }
    }
    fn source_read(&self, start: usize, limit: usize) -> Result<(usize,String)> {
        match self.source.read(start, limit) {
            AccessibleRead::Ready { start: actual, text } => {
                if actual < start || actual.saturating_add(text.len()) > start.saturating_add(limit) {return Err(unavailable());}
                Ok((actual,text))
            }
            AccessibleRead::Pending => Err(pending()),
            AccessibleRead::Unavailable => Err(unavailable()),
        }
    }
    /// One bounded source read plus the already-owned preedit. The virtual text
    /// remains continuous across both composition seams without reading a file
    /// into memory or reporting the seam as the end of the document.
    fn read(&self, start: usize, limit: usize) -> Result<(usize, String)> {
        if start > self.len() { return Err(invalid()); }
        let limit = limit.min(self.page).min(LIMIT);
        if let Some(o) = &self.overlay {
            if start < o.start {
                let (actual,mut text)=self.source_read(start,limit)?;
                if actual<=o.start && actual+text.len()>=o.start {text.insert_str(o.start-actual,&o.text);}
                let mut end=text.len().min(limit);
                while !text.is_char_boundary(end) {end-=1;}
                text.truncate(end);
                return Ok((actual,text));
            }
            if start >= o.start && start < o.start + o.text.len() {
                let mut a = start-o.start;
                let mut b = (a+limit).min(o.text.len());
                while a < b && !o.text.is_char_boundary(a) { a+=1; }
                while b > a && !o.text.is_char_boundary(b) { b-=1; }
                let mut text=o.text[a..b].to_owned();
                if b==o.text.len() && text.len()<limit {
                    let (_,tail)=self.source_read(o.end,limit-text.len())?;
                    text.push_str(&tail);
                }
                return Ok((o.start+a,text));
            }
            let committed=self.committed(start)?;
            let (actual,text)=self.source_read(committed,limit)?;
            return Ok((start+(actual-committed),text));
        }
        self.source_read(start,limit)
    }
}
#[implement(ITextProvider, ITextProvider2, ITextEditProvider)]
struct Provider { life: Weak<Life>, enclosing: IRawElementProviderSimple }
impl Provider {
    fn range(&self, start: usize, end: usize, view: &View) -> Result<ITextRangeProvider> {
        // Offsets and identity must come from the same captured view, even if
        // publication changes between this method and the preceding point read.
        self.life.upgrade().ok_or_else(unavailable)?;
        if start > end || end > view.len() { return Err(invalid()); }
        Ok(TextRange { life: self.life.clone(), enclosing: self.enclosing.clone(), source_token: view.source.identity(), overlay: view.overlay.clone(), endpoints: Mutex::new((start,end)) }.into())
    }
}
impl ITextProvider_Impl for Provider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        let (a,c) = view.selection;
        let range = self.range(view.virtual_offset(a.min(c)), view.virtual_offset(a.max(c)), &view)?;
        array(&[range.cast()?])
    }
    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        let spans=if view.visible_ranges.is_empty() {
            vec![(view.virtual_offset(view.visible.0),view.virtual_offset(view.visible.1).min(view.len()))]
        } else {view.visible_ranges.clone()};
        let ranges=spans.into_iter().map(|(start,end)|self.range(start,end,&view)?.cast()).collect::<Result<Vec<IUnknown>>>()?;
        array(&ranges)
    }
    fn RangeFromChild(&self, _child: Ref<IRawElementProviderSimple>) -> Result<ITextRangeProvider> { Err(invalid()) }
    fn RangeFromPoint(&self, point: &UiaPoint) -> Result<ITextRangeProvider> {
        if !point.x.is_finite() || !point.y.is_finite() { return Err(invalid()); }
        let life = self.life.upgrade().ok_or_else(unavailable)?;
        let view = life.view()?;
        if view.len() == 0 { return self.range(0,0,&view); }
        let boxes = life.geometry(&view, &self.enclosing)?;
        let closest = boxes.iter().min_by(|a,b| {
            let distance = |r: &AccessibilityTextBox| {
                let [x,y,w,h] = r.bounds;
                let dx = point.x-point.x.clamp(x,x+w);
                let dy = point.y-point.y.clamp(y,y+h);
                dx*dx+dy*dy
            };
            distance(a).total_cmp(&distance(b))
        }).ok_or_else(unavailable)?;
        self.range(closest.start,closest.start,&view)
    }
    fn DocumentRange(&self) -> Result<ITextRangeProvider> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        self.range(0, view.len(), &view)
    }
    fn SupportedTextSelection(&self) -> Result<SupportedTextSelection> { Ok(SupportedTextSelection_Single) }
}
impl ITextProvider2_Impl for Provider_Impl {
    fn RangeFromAnnotation(&self, _annotation: Ref<IRawElementProviderSimple>) -> Result<ITextRangeProvider> { Err(invalid()) }
    fn GetCaretRange(&self, active: *mut BOOL) -> Result<ITextRangeProvider> {
        if active.is_null() { return Err(invalid()); }
        let life = self.life.upgrade().ok_or_else(unavailable)?;
        let view = life.view()?;
        let focused = life.shared.lock().unwrap_or_else(|e| e.into_inner()).snapshot.focus == 2;
        // SAFETY: COM out pointer was checked and is valid for this call.
        unsafe { *active = focused.into(); }
        let caret = view.overlay.as_ref().map_or(view.virtual_offset(view.selection.1), |o| o.start+o.text.len());
        self.range(caret,caret,&view)
    }
}
impl ITextEditProvider_Impl for Provider_Impl {
    fn GetActiveComposition(&self) -> Result<ITextRangeProvider> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        let Some(o) = &view.overlay else { return Err(Error::empty()); };
        self.range(o.start, o.start+o.text.len(), &view)
    }
    fn GetConversionTarget(&self) -> Result<ITextRangeProvider> {
        // A preedit cursor is not a conversion target. winit does not expose the
        // IME's target clause; UIA specifies a null result when none is available.
        self.life.upgrade().ok_or_else(unavailable)?.view()?;
        Err(Error::empty())
    }
}
#[implement(ITextRangeProvider)]
struct TextRange {
    life: Weak<Life>, enclosing: IRawElementProviderSimple,
    source_token: (u64,u64), overlay: Option<Overlay>, endpoints: Mutex<(usize,usize)>,
}
impl TextRange {
    fn view(&self) -> Result<View> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        if view.source.identity() != self.source_token || view.overlay != self.overlay { return Err(unavailable()); }
        Ok(view)
    }
    fn endpoints(&self) -> (usize,usize) { *self.endpoints.lock().unwrap_or_else(|e| e.into_inner()) }
    fn other(&self, other: Ref<ITextRangeProvider>) -> Result<(usize,usize)> {
        let other = other.as_ref().ok_or_else(invalid)?.cast_object_ref::<TextRange>().map_err(|_| invalid())?;
        if !self.life.ptr_eq(&other.life) || self.source_token != other.source_token || self.overlay != other.overlay { return Err(invalid()); }
        other.view()?;
        Ok(other.endpoints())
    }
    fn new_range(&self, start: usize, end: usize) -> ITextRangeProvider {
        TextRange { life: self.life.clone(), enclosing: self.enclosing.clone(), source_token: self.source_token, overlay: self.overlay.clone(), endpoints: Mutex::new((start,end)) }.into()
    }
    fn set(&self, endpoint: TextPatternRangeEndpoint, position: usize) -> Result<()> {
        let mut range = self.endpoints.lock().unwrap_or_else(|e| e.into_inner());
        match endpoint {
            TextPatternRangeEndpoint_Start => { range.0=position; range.1=range.1.max(position); },
            TextPatternRangeEndpoint_End => { range.1=position; range.0=range.0.min(position); },
            _ => return Err(invalid()),
        }
        Ok(())
    }
}
fn endpoint(range: (usize,usize), endpoint: TextPatternRangeEndpoint) -> Result<usize> {
    match endpoint { TextPatternRangeEndpoint_Start => Ok(range.0), TextPatternRangeEndpoint_End => Ok(range.1), _ => Err(invalid()) }
}
fn positions(view: &View, at: usize, unit: TextUnit) -> Result<Vec<usize>> {
    if matches!(unit, TextUnit_Document | TextUnit_Page) { return Ok(vec![0,view.len()]); }
    let start = at.saturating_sub(view.page/2);
    let (start,text) = view.read(start,view.page)?;
    if matches!(unit,TextUnit_Character|TextUnit_Format) {
        if at<start || at>start+text.len() {return Err(unsupported());}
        // Page targets are byte estimates. Start cursor analysis at the next
        // scalar boundary, then choose a proven grapheme boundary below.
        let mut at=at;
        while at<start+text.len() && !text.is_char_boundary(at-start) {at+=1;}
        let mut offsets=Vec::new();
        let mut cursor=unicode_segmentation::GraphemeCursor::new(at,view.len(),true);
        match cursor.is_boundary(&text,start) {
            Ok(true)=>offsets.push(at),Ok(false)=>(),Err(_)=>return Err(unsupported()),
        }
        let mut forward=unicode_segmentation::GraphemeCursor::new(at,view.len(),true);
        while let Ok(Some(offset))=forward.next_boundary(&text,start) {offsets.push(offset);}
        let mut backward=unicode_segmentation::GraphemeCursor::new(at,view.len(),true);
        while let Ok(Some(offset))=backward.prev_boundary(&text,start) {offsets.push(offset);}
        offsets.sort_unstable();offsets.dedup();
        return Ok(offsets);
    }
    let mut positions: Vec<_> = match unit {
        TextUnit_Word => text.split_word_bound_indices().map(|(i,_)| start+i).collect(),
        TextUnit_Line | TextUnit_Paragraph => {
            let mut values = vec![start];
            values.extend(text.grapheme_indices(true).filter(|(_,g)| matches!(*g,"\n"|"\r"|"\r\n")).map(|(i,g)| start+i+g.len())); values
        },
        _ => return Err(invalid()),
    };
    // A window edge in the middle of a grapheme/word/line is not a boundary.
    if start != 0 { positions.retain(|p| *p != start); }
    if start+text.len() == view.len() { positions.push(view.len()); }
    positions.sort_unstable(); positions.dedup();
    Ok(positions)
}
fn page_position(view: &View, at: usize, forward: bool) -> Result<usize> {
    let target=if forward {at.saturating_add(view.page).min(view.len())} else {at.saturating_sub(view.page)};
    if target==0 || target==view.len() {return Ok(target);}
    let boundaries=positions(view,target,TextUnit_Character)?;
    (if forward {boundaries.into_iter().find(|p|*p>=target)} else {boundaries.into_iter().rev().find(|p|*p<=target)}).ok_or_else(unsupported)
}
/// Fold a bounded window while retaining original UTF-8 scalar boundaries.
/// Expanded folds (ß→ss, ligatures) never produce half-scalar endpoints.
fn find_literal(value: &str, needle: &str, backward: bool, ignore_case: bool) -> Option<(usize,usize)> {
    if !ignore_case {
        return (if backward { value.rfind(needle) } else { value.find(needle) }).map(|start| (start,start+needle.len()));
    }
    let mut folded = String::new();
    let mut boundaries = Vec::new();
    let mut buffer = [0u8; 12];
    for (offset,c) in value.char_indices() {
        boundaries.push((folded.len(),offset));
        folded.push_str(bareline_unicode_fold::character(c,&mut buffer));
    }
    boundaries.push((folded.len(),value.len()));
    let needle = bareline_unicode_fold::fold(needle);
    if needle.is_empty() { return None; }
    let original = |start: usize| {
        let a = boundaries.binary_search_by_key(&start, |p|p.0).ok()?;
        let b = boundaries.binary_search_by_key(&(start+needle.len()), |p|p.0).ok()?;
        Some((boundaries[a].1,boundaries[b].1))
    };
    let pattern = needle.as_bytes();
    let mut prefix = vec![0;pattern.len()];
    for index in 1..pattern.len() {
        let mut matched = prefix[index-1];
        while matched>0 && pattern[index]!=pattern[matched] {matched=prefix[matched-1];}
        if pattern[index]==pattern[matched] {matched+=1;}
        prefix[index]=matched;
    }
    let mut matched=0;
    let mut found=None;
    for (index,byte) in folded.bytes().enumerate() {
        while matched>0 && byte!=pattern[matched] {matched=prefix[matched-1];}
        if byte==pattern[matched] {matched+=1;}
        if matched==pattern.len() {
            if let Some(range)=original(index+1-pattern.len()) {
                if !backward {return Some(range);}
                found=Some(range);
            }
            matched=prefix[matched-1];
        }
    }
    found
}
impl ITextRangeProvider_Impl for TextRange_Impl {
    fn Clone(&self) -> Result<ITextRangeProvider> { self.view()?; let (a,b)=self.endpoints(); Ok(self.new_range(a,b)) }
    fn Compare(&self, other: Ref<ITextRangeProvider>) -> Result<BOOL> { self.view()?; Ok((self.endpoints()==self.other(other)?).into()) }
    fn CompareEndpoints(&self, e: TextPatternRangeEndpoint, other: Ref<ITextRangeProvider>, oe: TextPatternRangeEndpoint) -> Result<i32> {
        self.view()?; Ok(match endpoint(self.endpoints(),e)?.cmp(&endpoint(self.other(other)?,oe)?) { std::cmp::Ordering::Less=>-1,std::cmp::Ordering::Equal=>0,std::cmp::Ordering::Greater=>1 })
    }
    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> Result<()> {
        let view=self.view()?; let at=self.endpoints().0;
        if unit==TextUnit_Document { *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(0,view.len()); return Ok(()); }
        if unit==TextUnit_Page {
            let boundaries=positions(&view,at,TextUnit_Character)?;
            let a=*boundaries.first().ok_or_else(unsupported)?;
            let b=*boundaries.last().ok_or_else(unsupported)?;
            *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(a,b);return Ok(());
        }
        if at==view.len() { return Ok(()); }
        let positions=positions(&view,at,unit)?;
        let a=positions.iter().rev().copied().find(|p|*p<=at).ok_or_else(unsupported)?;
        let b=positions.iter().copied().find(|p|*p>at).ok_or_else(unsupported)?;
        *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(a,b); Ok(())
    }
    fn FindAttribute(&self, _id: UIA_TEXTATTRIBUTE_ID, _value: &VARIANT, _backward: BOOL) -> Result<ITextRangeProvider> { Err(unsupported()) }
    fn FindText(&self, text: &BSTR, backward: BOOL, ignore_case: BOOL) -> Result<ITextRangeProvider> {
        let view=self.view()?; let (a,b)=self.endpoints();
        let (start,value)=view.read(if backward.as_bool() { b.saturating_sub(view.page).max(a) } else { a }, b.saturating_sub(a))?;
        if text.len() > LIMIT { return Err(invalid()); }
        let needle=text.to_string(); if needle.is_empty() || needle.len() > LIMIT { return Err(invalid()); }
        match find_literal(&value,&needle,backward.as_bool(),ignore_case.as_bool()) {
            Some((a,b))=>Ok(self.new_range(start+a,start+b)),
            None if start>a || start+value.len()<b => Err(unsupported()),
            None=>Err(Error::empty()),
        }
    }
    fn GetAttributeValue(&self, _id: UIA_TEXTATTRIBUTE_ID) -> Result<VARIANT> { self.view()?; Ok(unsafe { UiaGetReservedNotSupportedValue()? }.into()) }
    fn GetBoundingRectangles(&self) -> Result<*mut SAFEARRAY> {
        let view = self.view()?;
        let (start,end) = self.endpoints();
        let life = self.life.upgrade().ok_or_else(unavailable)?;
        let boxes = life.geometry(&view, &self.enclosing)?;
        let mut result = Vec::new();
        for rect in boxes {
            if rect.start < end && rect.end > start { result.extend(rect.bounds); }
        }
        rectangles(&result)
    }
    fn GetEnclosingElement(&self) -> Result<IRawElementProviderSimple> { self.view()?; Ok(self.enclosing.clone()) }
    fn GetText(&self, max_length: i32) -> Result<BSTR> {
        if max_length < -1 { return Err(invalid()); }
        let view=self.view()?; let (a,b)=self.endpoints();
        if a==b || max_length==0 { return Ok(BSTR::new()); }
        let (_,text)=view.read(a,b-a)?;
        let units: Vec<u16>=text.encode_utf16().collect();
        let mut n=if max_length<0 {units.len()} else {units.len().min(max_length as usize)};
        if n>0 && n<units.len() && (0xD800..=0xDBFF).contains(&units[n-1]) {n-=1;}
        Ok(BSTR::from_wide(&units[..n]))
    }
    fn Move(&self, unit: TextUnit, count: i32) -> Result<i32> {
        let before=self.endpoints();
        let temporary=self.new_range(before.0,before.1);
        let moved=unsafe {temporary.MoveEndpointByUnit(TextPatternRangeEndpoint_Start,unit,count)}?;
        if moved!=0 {
            let state=temporary.cast_object_ref::<TextRange>()?;
            let at=state.endpoints().0;
            *state.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(at,at);
            if before.0!=before.1 {unsafe {temporary.ExpandToEnclosingUnit(unit)}?;}
            *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=state.endpoints();
        }
        Ok(moved)
    }
    fn MoveEndpointByUnit(&self, e: TextPatternRangeEndpoint, unit: TextUnit, count: i32) -> Result<i32> {
        let view=self.view()?; let at=endpoint(self.endpoints(),e)?;
        if count==0 {return Ok(0);}
        if unit==TextUnit_Document {let next=if count>0 {view.len()} else {0}; self.set(e,next)?;return Ok(if next==at {0} else {count.signum()});}
        if unit==TextUnit_Page {
            let actual=page_position(&view,at,count>0)?;
            self.set(e,actual)?;return Ok(if actual==at {0} else {count.signum()});
        }
        let values=positions(&view,at,unit)?;
        let candidates:Vec<_>=if count>0 {values.into_iter().filter(|p|*p>at).collect()} else {values.into_iter().rev().filter(|p|*p<at).collect()};
        let n=candidates.len().min(count.unsigned_abs() as usize);
        if n==0 && ((count>0 && at<view.len()) || (count<0 && at>0)) {return Err(unsupported());}
        if n>0 {self.set(e,candidates[n-1])?;} Ok((n as i32)*count.signum())
    }
    fn MoveEndpointByRange(&self, e: TextPatternRangeEndpoint, other: Ref<ITextRangeProvider>, oe: TextPatternRangeEndpoint) -> Result<()> {self.view()?;self.set(e,endpoint(self.other(other)?,oe)?)}
    fn Select(&self) -> Result<()> {let view=self.view()?;let(a,b)=self.endpoints();self.life.upgrade().ok_or_else(unavailable)?.action(AccessibilityAction::SetSelection{source_identity:view.source.identity(),anchor:view.committed(a)?,caret:view.committed(b)?})}
    fn AddToSelection(&self) -> Result<()> {Err(unsupported())}
    fn RemoveFromSelection(&self) -> Result<()> {Err(unsupported())}
    fn ScrollIntoView(&self, top: BOOL) -> Result<()> {let view=self.view()?;let(a,b)=self.endpoints(); self.life.upgrade().ok_or_else(unavailable)?.action(AccessibilityAction::ScrollToText{source_identity:view.source.identity(),offset:view.committed(if top.as_bool(){a}else{b})?})}
    fn GetChildren(&self) -> Result<*mut SAFEARRAY> {self.view()?;array(&[])}
}

#[cfg(test)]
mod identity_tests {
    use super::*;
    #[implement(IRawElementProviderSimple)]
    struct Enclosing;
    impl IRawElementProviderSimple_Impl for Enclosing_Impl {
        fn ProviderOptions(&self)->Result<ProviderOptions>{Ok(ProviderOptions_ServerSideProvider)}
        fn GetPatternProvider(&self,_:UIA_PATTERN_ID)->Result<IUnknown>{Err(Error::empty())}
        fn GetPropertyValue(&self,_:UIA_PROPERTY_ID)->Result<VARIANT>{Ok(VARIANT::default())}
        fn HostRawElementProvider(&self)->Result<IRawElementProviderSimple>{Err(Error::empty())}
    }
    fn provider(text:&'static str,composition:Option<&str>)->(Arc<Life>,ITextEditProvider){
        let life=Arc::new(life());
        *life.source.write().unwrap()=Some(Arc::new(Text(text)));
        *life.shared.lock().unwrap().snapshot.text_context.as_mut().unwrap()=AccessibilityTextContext {
            source_identity:(90,1),selection:(1,1),composition:composition.map(str::to_owned),
        };
        let provider=Provider{life:Arc::downgrade(&life),enclosing:Enclosing.into()}.into();
        (life,provider)
    }
    #[test]
    fn com_ranges_clone_move_select_and_reject_stale_or_foreign_sources(){
        let (life,provider)=provider("aé👩‍💻z",None);
        let pattern:ITextProvider=provider.cast().unwrap();
        // Calls are in-process COM with live fixture interfaces and valid out
        // parameters generated by windows-rs; no native window or UIA client.
        let range=unsafe{pattern.DocumentRange()}.unwrap();
        let clone=unsafe{range.Clone()}.unwrap();
        assert!(unsafe{range.Compare(&clone)}.unwrap().as_bool());
        assert_eq!(unsafe{range.GetText(2)}.unwrap().to_string(),"aé");
        assert_eq!(unsafe{range.MoveEndpointByUnit(TextPatternRangeEndpoint_End,TextUnit_Character,-1)}.unwrap(),-1);
        unsafe{range.Select()}.unwrap();
        assert_eq!(life.shared.lock().unwrap().actions,vec![AccessibilityAction::SetSelection{source_identity:(90,1),anchor:0,caret:14}]);
        let (_other,other_provider)=provider_fixture("other");
        let foreign=unsafe{other_provider.DocumentRange()}.unwrap();
        assert!(unsafe{range.Compare(&foreign)}.is_err());
        *life.source.write().unwrap()=Some(Arc::new(Source((90,2))));
        assert!(unsafe{range.GetText(-1)}.is_err());
        drop(life);
        assert!(unsafe{clone.GetText(-1)}.is_err());
    }
    fn provider_fixture(text:&'static str)->(Arc<Life>,ITextProvider){
        let (life,provider)=provider(text,None);(life,provider.cast().unwrap())
    }
    #[test]
    fn com_visible_ranges_preserve_source_mapped_footer_across_large_fold() {
        // Only the header is cached. Retained geometry maps the footer past a
        // hidden body larger than the paged viewport; UIA must not expose that
        // hidden body as visible or substitute header bytes for the footer.
        let hidden=300*1024;
        let text: &'static str=Box::leak(format!("HEAD{}FOOT", "x".repeat(hidden)).into_boxed_str());
        let footer=4+hidden;
        let (life,pattern)=provider_fixture(text);
        {
            let mut shared=life.shared.lock().unwrap();
            shared.snapshot.text=Some(AccessibilityText {editor_id:2,run_id:u64::MAX,start_byte:0,value:"HEAD".into(),character_lengths:vec![1;4],selection:None});
            // Reordered and adjacent glyph boxes exercise normalization; only
            // adjacency in global source offsets permits merging.
            shared.snapshot.text_geometry=vec![
                AccessibilityTextBox{start:footer+2,end:footer+4,bounds:[20.,40.,20.,20.]},
                AccessibilityTextBox{start:0,end:2,bounds:[0.,0.,20.,20.]},
                AccessibilityTextBox{start:footer,end:footer+2,bounds:[0.,40.,20.,20.]},
                AccessibilityTextBox{start:2,end:4,bounds:[20.,0.,20.,20.]},
            ];
        }
        let array=unsafe{pattern.GetVisibleRanges()}.unwrap();
        let result=(||->Result<Vec<ITextRangeProvider>> {
            use windows::Win32::System::Ole::{SafeArrayGetElement,SafeArrayGetUBound};
            let upper=unsafe{SafeArrayGetUBound(array,1)}?;
            let mut ranges=Vec::new();
            for index in 0..=upper {
                let mut raw=std::ptr::null_mut();
                // VT_UNKNOWN GetElement returns an owned AddRef. from_raw
                // transfers it to the RAII interface before the SAFEARRAY dies.
                unsafe{SafeArrayGetElement(array,&index,(&mut raw as *mut *mut std::ffi::c_void).cast())}?;
                let unknown=unsafe{IUnknown::from_raw(raw)};
                ranges.push(unknown.cast()?);
            }
            Ok(ranges)
        })();
        unsafe{SafeArrayDestroy(array)}.unwrap();
        let ranges=result.unwrap();
        assert_eq!(ranges.len(),2);
        assert_eq!(unsafe{ranges[0].GetText(-1)}.unwrap().to_string(),"HEAD");
        assert_eq!(unsafe{ranges[1].GetText(-1)}.unwrap().to_string(),"FOOT");
        let document=unsafe{pattern.DocumentRange()}.unwrap();
        assert_eq!(ranges[0].cast_object_ref::<TextRange>().unwrap().endpoints(),(0,4));
        assert_eq!(ranges[1].cast_object_ref::<TextRange>().unwrap().endpoints(),(footer,footer+4));
        assert_eq!(unsafe{ranges[1].CompareEndpoints(TextPatternRangeEndpoint_Start,&document,TextPatternRangeEndpoint_Start)}.unwrap(),1);
        assert_eq!(unsafe{ranges[1].CompareEndpoints(TextPatternRangeEndpoint_End,&document,TextPatternRangeEndpoint_End)}.unwrap(),0);
        life.shared.lock().unwrap().snapshot.text_context.as_mut().unwrap().source_identity=(90,2);
        assert!(unsafe{pattern.GetVisibleRanges()}.is_err());
        assert!(unsafe{ranges[1].GetText(-1)}.is_err());
    }
    #[test]
    fn com_active_composition_and_virtual_document_share_offsets(){
        let (_life,provider)=provider("ab",Some("界"));
        let active=unsafe{provider.GetActiveComposition()}.unwrap();
        assert_eq!(unsafe{active.GetText(-1)}.unwrap().to_string(),"界");
        let pattern:ITextProvider=provider.cast().unwrap();
        let document=unsafe{pattern.DocumentRange()}.unwrap();
        assert_eq!(unsafe{document.GetText(-1)}.unwrap().to_string(),"a界b");
        assert_eq!(unsafe{active.CompareEndpoints(TextPatternRangeEndpoint_Start,&document,TextPatternRangeEndpoint_Start)}.unwrap(),1);
    }
    struct Text(&'static str);
    impl AccessibilityTextSource for Text {
        fn identity(&self)->(u64,u64){(90,1)}
        fn len(&self)->usize{self.0.len()}
        fn read(&self,start:usize,limit:usize)->AccessibleRead{
            assert!(limit<=LIMIT);
            let mut a=start;let mut b=(start+limit).min(self.len());
            while a<b && !self.0.is_char_boundary(a){a+=1;}
            while b>a && !self.0.is_char_boundary(b){b-=1;}
            AccessibleRead::Ready{start:a,text:self.0[a..b].into()}
        }
    }
    #[test]
    fn unicode_find_preserves_expanded_original_boundaries_and_overlap() {
        assert_eq!(find_literal("sß","SS",false,true),Some((1,3)));
        assert_eq!(find_literal("ß","s",false,true),None);
        assert_eq!(find_literal("Straße Σς","STRASSE",false,true),Some((0,7)));
        assert_eq!(find_literal("Σς","σ",true,true),Some((2,4)));
        assert_eq!(find_literal("aaa","aa",true,false),Some((1,3)));
    }
    #[test]
    fn virtual_preedit_read_crosses_both_seams_without_changing_source() {
        let view=View{source:Arc::new(Text("ab")),overlay:Some(Overlay{start:1,end:1,text:"界".into()}),selection:(1,1),visible:(0,2),visible_ranges:vec![],page:1024};
        assert_eq!(view.read(0,LIMIT).unwrap(),(0,"a界b".into()));
        assert_eq!(view.read(1,4).unwrap(),(1,"界b".into()));
        assert_eq!(view.read(4,1).unwrap(),(4,"b".into()));
        assert_eq!(view.source.len(),2);
        assert_eq!(view.committed(4).unwrap(),1);
        assert!(view.committed(2).is_err());
    }
    #[test]
    fn page_targets_never_split_combining_or_emoji_clusters(){
        let view=View{source:Arc::new(Text("a👩‍💻e\u{301}xyz")),overlay:None,selection:(0,0),visible:(0,4),visible_ranges:vec![],page:4};
        // The cluster is larger than the bounded context; report unsupported,
        // rather than manufacture an endpoint inside the ZWJ sequence.
        assert!(page_position(&view,0,true).is_err());
        let view=View{source:Arc::new(Text("ae\u{301}xyz")),overlay:None,selection:(0,0),visible:(0,3),visible_ranges:vec![],page:3};
        let at=page_position(&view,0,true).unwrap();
        assert_eq!(at,4);
    }
    #[test]
    fn five_gib_range_window_is_bounded_at_an_offscreen_offset() {
        struct Generated;
        impl AccessibilityTextSource for Generated {
            fn identity(&self)->(u64,u64){(91,1)}
            fn len(&self)->usize{5*1024*1024*1024}
            fn read(&self,start:usize,limit:usize)->AccessibleRead {
                assert!(limit<=2048);
                AccessibleRead::Ready{start,text:"x".repeat(limit.min(self.len()-start))}
            }
        }
        let view=View{source:Arc::new(Generated),overlay:None,selection:(0,0),visible:(0,2048),visible_ranges:vec![],page:2048};
        let start=4*1024*1024*1024;
        let (at,text)=view.read(start,usize::MAX).unwrap();
        assert_eq!(at,start);assert_eq!(text.len(),2048);
        let offsets=positions(&view,start,TextUnit_Character).unwrap();
        assert!(offsets.iter().any(|offset|*offset>start));
        assert!(offsets.len()<=2048);
    }
    struct Source((u64, u64));
    impl AccessibilityTextSource for Source {
        fn identity(&self) -> (u64, u64) { self.0 }
        fn len(&self) -> usize { 20 }
        fn read(&self, _: usize, _: usize) -> AccessibleRead { panic!("identity checks must not read text") }
    }
    fn life() -> Life {
        Life {
            source: RwLock::new(Some(Arc::new(Source((10, 1))))),
            shared: Arc::new(Mutex::new(Shared {
                snapshot: AccessibilitySnapshot {
                    root: 1, focus: 2, nodes: vec![], text: None, text_geometry: vec![],
                    text_context: Some(AccessibilityTextContext {
                        source_identity: (10, 1), selection: (2, 4), composition: None,
                    }),
                }, actions: vec![],
            })),
            notify: Arc::new(|| {}),
        }
    }
    #[test]
    fn mixed_source_and_selection_publication_is_unavailable() {
        let life = life();
        assert!(life.view().is_ok());
        *life.source.write().unwrap() = Some(Arc::new(Source((11, 0))));
        assert_eq!(life.view().err().unwrap().code(), unavailable().code());
        life.shared.lock().unwrap().snapshot.text_context.as_mut().unwrap().source_identity = (11, 0);
        assert!(life.view().is_ok());
        // An edit of the same source also invalidates the pair.
        *life.source.write().unwrap() = Some(Arc::new(Source((11, 1))));
        assert!(life.view().is_err());
    }
    #[test]
    fn queued_text_actions_retain_the_validated_revision() {
        let life = life();
        let view = life.view().unwrap();
        life.action(AccessibilityAction::SetSelection {
            source_identity: view.source.identity(), anchor: 2, caret: 4,
        }).unwrap();
        life.action(AccessibilityAction::ScrollToText {
            source_identity: view.source.identity(), offset: 4,
        }).unwrap();
        *life.source.write().unwrap() = Some(Arc::new(Source((20, 0))));
        let shared = life.shared.lock().unwrap();
        assert_eq!(shared.actions, vec![
            AccessibilityAction::SetSelection { source_identity: (10, 1), anchor: 2, caret: 4 },
            AccessibilityAction::ScrollToText { source_identity: (10, 1), offset: 4 },
        ]);
    }
}
