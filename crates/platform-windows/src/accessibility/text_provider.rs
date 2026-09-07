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
    page: usize,
}
impl Life {
    fn geometry(&self, view: &View, enclosing: &IRawElementProviderSimple) -> Result<Vec<AccessibilityTextBox>> {
        let (mut boxes, origin) = {
            let shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if shared.snapshot.text_context.as_ref().map(|c| c.source_identity) != Some(view.source.identity()) { return Err(unavailable()); }
            // Committed layouts do not describe the IME overlay. Do not publish
            // misleading rectangles while composition replaces those glyphs.
            if view.overlay.is_some() { return Ok(Vec::new()); }
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
        // Source and tree publication can straddle a UIA call. A mixed pair is
        // unavailable, never a new document with the old document's selection.
        if context.source_identity != source.identity() { return Err(unavailable()); }
        let (a, c) = context.selection;
        if a > source.len() || c > source.len() { return Err(unavailable()); }
        let overlay = context.composition.as_ref().map(|text| Overlay { start: a.min(c), end: a.max(c), text: text.clone() });
        let visible = shared.snapshot.text.as_ref().map(|t| (t.start_byte, t.start_byte + t.value.len())).unwrap_or((c,c));
        Ok(View { source, overlay, selection: (a,c), visible, page: visible.1.saturating_sub(visible.0).clamp(1024, LIMIT) })
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
    /// A virtual preedit segment is part of the same provider text domain. Read
    /// stops at its seam, so crossing a huge replaced selection never reads it.
    fn read(&self, start: usize, limit: usize) -> Result<(usize, String)> {
        if start > self.len() { return Err(invalid()); }
        let limit = limit.min(self.page).min(LIMIT);
        if let Some(o) = &self.overlay {
            if start >= o.start && start < o.start + o.text.len() {
                let mut a = start-o.start;
                let mut b = (a+limit).min(o.text.len());
                while a < b && !o.text.is_char_boundary(a) { a+=1; }
                while b > a && !o.text.is_char_boundary(b) { b-=1; }
                return Ok((o.start+a, o.text[a..b].to_owned()));
            }
        }
        let committed = self.committed(start)?;
        let limit = self.overlay.as_ref().filter(|o| start < o.start).map_or(limit, |o| limit.min(o.start-start));
        match self.source.read(committed, limit) {
            AccessibleRead::Ready { start: actual, text } => {
                if text.len() > limit || actual < committed || actual > committed.saturating_add(limit) { return Err(unavailable()); }
                Ok((self.virtual_offset(actual), text))
            },
            AccessibleRead::Pending => Err(pending()),
            AccessibleRead::Unavailable => Err(unavailable()),
        }
    }
}
#[implement(ITextProvider, ITextProvider2, ITextEditProvider)]
struct Provider { life: Weak<Life>, enclosing: IRawElementProviderSimple }
impl Provider {
    fn range(&self, start: usize, end: usize) -> Result<ITextRangeProvider> {
        let life = self.life.upgrade().ok_or_else(unavailable)?;
        let view = life.view()?;
        if start > end || end > view.len() { return Err(invalid()); }
        Ok(TextRange { life: self.life.clone(), enclosing: self.enclosing.clone(), source_token: view.source.identity(), overlay: view.overlay, endpoints: Mutex::new((start,end)) }.into())
    }
}
impl ITextProvider_Impl for Provider_Impl {
    fn GetSelection(&self) -> Result<*mut SAFEARRAY> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        let (a,c) = view.selection;
        let range = self.range(view.virtual_offset(a.min(c)), view.virtual_offset(a.max(c)))?;
        array(&[range.cast()?])
    }
    fn GetVisibleRanges(&self) -> Result<*mut SAFEARRAY> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        array(&[self.range(view.virtual_offset(view.visible.0), view.virtual_offset(view.visible.1).min(view.len()))?.cast()?])
    }
    fn RangeFromChild(&self, _child: Ref<IRawElementProviderSimple>) -> Result<ITextRangeProvider> { Err(invalid()) }
    fn RangeFromPoint(&self, point: &UiaPoint) -> Result<ITextRangeProvider> {
        if !point.x.is_finite() || !point.y.is_finite() { return Err(invalid()); }
        let life = self.life.upgrade().ok_or_else(unavailable)?;
        let view = life.view()?;
        if view.len() == 0 { return self.range(0,0); }
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
        self.range(closest.start,closest.start)
    }
    fn DocumentRange(&self) -> Result<ITextRangeProvider> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        self.range(0, view.len())
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
        self.range(caret,caret)
    }
}
impl ITextEditProvider_Impl for Provider_Impl {
    fn GetActiveComposition(&self) -> Result<ITextRangeProvider> {
        let view = self.life.upgrade().ok_or_else(unavailable)?.view()?;
        let Some(o) = view.overlay else { return Err(Error::empty()); };
        self.range(o.start, o.start+o.text.len())
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
    let mut positions: Vec<_> = match unit {
        TextUnit_Character | TextUnit_Format => text.grapheme_indices(true).map(|(i,_)| start+i).collect(),
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
impl ITextRangeProvider_Impl for TextRange_Impl {
    fn Clone(&self) -> Result<ITextRangeProvider> { self.view()?; let (a,b)=self.endpoints(); Ok(self.new_range(a,b)) }
    fn Compare(&self, other: Ref<ITextRangeProvider>) -> Result<BOOL> { self.view()?; Ok((self.endpoints()==self.other(other)?).into()) }
    fn CompareEndpoints(&self, e: TextPatternRangeEndpoint, other: Ref<ITextRangeProvider>, oe: TextPatternRangeEndpoint) -> Result<i32> {
        self.view()?; Ok(match endpoint(self.endpoints(),e)?.cmp(&endpoint(self.other(other)?,oe)?) { std::cmp::Ordering::Less=>-1,std::cmp::Ordering::Equal=>0,std::cmp::Ordering::Greater=>1 })
    }
    fn ExpandToEnclosingUnit(&self, unit: TextUnit) -> Result<()> {
        let view=self.view()?; let at=self.endpoints().0;
        if unit==TextUnit_Document { *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(0,view.len()); return Ok(()); }
        if unit==TextUnit_Page { let a=view.virtual_offset(view.visible.0); let b=view.virtual_offset(view.visible.1).min(view.len()); *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(a,b); return Ok(()); }
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
        // Unicode case folding can change offsets; exact bounded search is safe.
        if ignore_case.as_bool() { return Err(unsupported()); }
        let needle=text.to_string(); if needle.is_empty() { return Err(invalid()); }
        let found=if backward.as_bool() {value.rfind(&needle)} else {value.find(&needle)};
        match found {Some(i)=>Ok(self.new_range(start+i,start+i+needle.len())),None=>Err(Error::empty())}
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
        let moved=self.MoveEndpointByUnit(TextPatternRangeEndpoint_Start,unit,count)?;
        if moved!=0 { let at=self.endpoints().0; *self.endpoints.lock().unwrap_or_else(|e|e.into_inner())=(at,at); if before.0!=before.1 {self.ExpandToEnclosingUnit(unit)?;} }
        Ok(moved)
    }
    fn MoveEndpointByUnit(&self, e: TextPatternRangeEndpoint, unit: TextUnit, count: i32) -> Result<i32> {
        let view=self.view()?; let at=endpoint(self.endpoints(),e)?;
        if count==0 {return Ok(0);}
        if unit==TextUnit_Document {let next=if count>0 {view.len()} else {0}; self.set(e,next)?;return Ok(if next==at {0} else {count.signum()});}
        if unit==TextUnit_Page {
            let candidate=if count>0 {at.saturating_add(view.page).min(view.len())} else {at.saturating_sub(view.page)};
            let (actual,_)=view.read(candidate,4)?; self.set(e,actual)?; return Ok(if actual==at {0} else {count.signum()});
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
