// SPDX-License-Identifier: MPL-2.0
#include <algorithm>
#include <cstdint>
#include <cstring>
#include <memory>
#include <stdexcept>
#include <vector>
#include "ILexer.h"
#include "LexerModule.h"
using namespace Scintilla;
extern const Lexilla::LexerModule lmCPP, lmPython, lmRust, lmHTML, lmXML, lmCss, lmJSON, lmSQL, lmTOML;
namespace {
constexpr size_t quota = 256 * 1024;
using Cancel = int (*)(void *);
struct Stopped {};
class Document final : public IDocument {
    std::vector<char> text;
    std::vector<Sci_Position> starts{0};
    Sci_Position cursor = 0;
    unsigned char seedStyle;
    int seedState;
    Cancel cancel;
    void *context;
    mutable size_t calls = 0;
public:
    std::vector<unsigned char> styles;
    std::vector<int> states, levels;
    mutable bool stopped = false;
    int error = 0;
    Document(const uint8_t *data, size_t size, unsigned char style, int state, Cancel cb, void *ctx)
        : text(data, data + size), seedStyle(style), seedState(state), cancel(cb), context(ctx), styles(size, 0) {
        for (size_t i = 0; i < size; ++i) {
            if (text[i] == '\r') { if (i + 1 < size && text[i + 1] == '\n') ++i; starts.push_back(i + 1); }
            else if (text[i] == '\n') starts.push_back(i + 1);
        }
        states.resize(starts.size(), 0); levels.resize(starts.size(), 0x400);
    }
    void check() const {
        if (++calls > quota * 256 || (cancel && cancel(context))) { stopped = true; throw Stopped{}; }
    }
    bool valid(Sci_Position p) const { return p >= 0 && static_cast<size_t>(p) < text.size(); }
    bool lineValid(Sci_Position n) const { return n >= 0 && static_cast<size_t>(n) < starts.size(); }
    int SCI_METHOD Version() const override { check(); return dvRelease4; }
    void SCI_METHOD SetErrorStatus(int status) override { error = status; }
    Sci_Position SCI_METHOD Length() const override { check(); return text.size(); }
    void SCI_METHOD GetCharRange(char *out, Sci_Position p, Sci_Position count) const override {
        check(); if (!out || p < 0 || count < 0 || static_cast<size_t>(p) > text.size() || static_cast<size_t>(count) > text.size() - p) throw std::out_of_range("range");
        if (count) std::memcpy(out, text.data() + p, count);
    }
    char SCI_METHOD StyleAt(Sci_Position p) const override { check(); return p < 0 ? seedStyle : valid(p) ? styles[p] : 0; }
    Sci_Position SCI_METHOD LineFromPosition(Sci_Position p) const override {
        check(); p = std::clamp<Sci_Position>(p, 0, text.size()); return std::upper_bound(starts.begin(), starts.end(), p) - starts.begin() - 1;
    }
    Sci_Position SCI_METHOD LineStart(Sci_Position n) const override { check(); return n < 0 ? 0 : lineValid(n) ? starts[n] : text.size(); }
    int SCI_METHOD GetLevel(Sci_Position n) const override { check(); return lineValid(n) ? levels[n] : 0x400; }
    int SCI_METHOD SetLevel(Sci_Position n, int value) override { check(); int old = GetLevel(n); if (lineValid(n)) levels[n] = value; return old; }
    int SCI_METHOD GetLineState(Sci_Position n) const override { check(); return n < 0 ? seedState : lineValid(n) ? states[n] : 0; }
    int SCI_METHOD SetLineState(Sci_Position n, int value) override { check(); int old = GetLineState(n); if (lineValid(n)) states[n] = value; return old; }
    void SCI_METHOD StartStyling(Sci_Position p) override { check(); if (p < 0 || static_cast<size_t>(p) > text.size()) throw std::out_of_range("style"); cursor = p; }
    bool SCI_METHOD SetStyleFor(Sci_Position count, char style) override {
        check(); if (count < 0 || static_cast<size_t>(count) > styles.size() - cursor) return false;
        std::fill_n(styles.begin() + cursor, count, static_cast<unsigned char>(style)); cursor += count; return true;
    }
    bool SCI_METHOD SetStyles(Sci_Position count, const char *values) override {
        check(); if (count < 0 || !values || static_cast<size_t>(count) > styles.size() - cursor) return false;
        std::copy_n(values, count, styles.begin() + cursor); cursor += count; return true;
    }
    void SCI_METHOD DecorationSetCurrentIndicator(int) override { check(); }
    void SCI_METHOD DecorationFillRange(Sci_Position, int, Sci_Position) override { check(); }
    void SCI_METHOD ChangeLexerState(Sci_Position, Sci_Position) override { check(); }
    int SCI_METHOD CodePage() const override { check(); return 65001; }
    bool SCI_METHOD IsDBCSLeadByte(char) const override { check(); return false; }
    const char *SCI_METHOD BufferPointer() override { check(); return text.data(); }
    int SCI_METHOD GetLineIndentation(Sci_Position n) override {
        check(); int column = 0; for (auto p = LineStart(n); valid(p); ++p) { if (text[p] == ' ') ++column; else if (text[p] == '\t') column = (column / 8 + 1) * 8; else break; } return column;
    }
    Sci_Position SCI_METHOD LineEnd(Sci_Position n) const override {
        check(); auto p = LineStart(n + 1); if (p > LineStart(n) && text[p - 1] == '\n') --p; if (p > LineStart(n) && text[p - 1] == '\r') --p; return p;
    }
    Sci_Position SCI_METHOD GetRelativePosition(Sci_Position p, Sci_Position offset) const override {
        check(); if (p < 0 || static_cast<size_t>(p) > text.size()) return -1;
        while (offset > 0) { check(); if (!valid(p)) return -1; ++p; while (valid(p) && (static_cast<unsigned char>(text[p]) & 0xc0) == 0x80) ++p; --offset; }
        while (offset < 0) { check(); if (p <= 0) return -1; --p; while (p > 0 && (static_cast<unsigned char>(text[p]) & 0xc0) == 0x80) --p; ++offset; } return p;
    }
    int SCI_METHOD GetCharacterAndWidth(Sci_Position p, Sci_Position *width) const override {
        check(); if (width) *width = 1; if (!valid(p)) return 0;
        auto first = static_cast<unsigned char>(text[p]); int size = first < 0x80 ? 1 : first < 0xe0 ? 2 : first < 0xf0 ? 3 : 4;
        if (static_cast<size_t>(p) + size > text.size()) return first;
        int value = first & (size == 1 ? 0x7f : (1 << (7 - size)) - 1);
        for (int i = 1; i < size; ++i) { auto next = static_cast<unsigned char>(text[p + i]); if ((next & 0xc0) != 0x80) return first; value = (value << 6) | (next & 0x3f); }
        if (width) *width = size; return value;
    }
};
struct Release { void operator()(ILexer5 *p) const { if (p) p->Release(); } };
// Translate every accessor to absolute coordinates. Reads before the retained
// window fail rather than supplying invented text or opaque lexer state.
struct Unavailable {};
class AbsoluteDocument final : public IDocument {
    Sci_Position origin, firstLine;
    Sci_Position position(Sci_Position p) const { if (origin != 0 && p < origin) throw Unavailable{}; return p - origin; }
    Sci_Position line(Sci_Position n) const { if (firstLine != 0 && n < firstLine) throw Unavailable{}; return n - firstLine; }
public:
    Document local;
    AbsoluteDocument(const uint8_t *data, size_t size, Sci_Position start, Sci_Position first, Cancel cb, void *ctx)
        : origin(start), firstLine(first), local(data, size, 0, 0, cb, ctx) {}
    int SCI_METHOD Version() const override { return local.Version(); }
    void SCI_METHOD SetErrorStatus(int s) override { local.SetErrorStatus(s); }
    Sci_Position SCI_METHOD Length() const override { return origin + local.Length(); }
    void SCI_METHOD GetCharRange(char *out, Sci_Position p, Sci_Position n) const override { local.GetCharRange(out, position(p), n); }
    char SCI_METHOD StyleAt(Sci_Position p) const override { return p < 0 ? 0 : local.StyleAt(position(p)); }
    Sci_Position SCI_METHOD LineFromPosition(Sci_Position p) const override { return firstLine + local.LineFromPosition(position(p)); }
    Sci_Position SCI_METHOD LineStart(Sci_Position n) const override { return n < 0 ? 0 : origin + local.LineStart(line(n)); }
    int SCI_METHOD GetLevel(Sci_Position n) const override { return n < 0 ? 0x400 : local.GetLevel(line(n)); }
    int SCI_METHOD SetLevel(Sci_Position n, int value) override { return local.SetLevel(line(n), value); }
    int SCI_METHOD GetLineState(Sci_Position n) const override { return n < 0 ? 0 : local.GetLineState(line(n)); }
    int SCI_METHOD SetLineState(Sci_Position n, int value) override { return local.SetLineState(line(n), value); }
    void SCI_METHOD StartStyling(Sci_Position p) override { local.StartStyling(position(p)); }
    bool SCI_METHOD SetStyleFor(Sci_Position n, char s) override { return local.SetStyleFor(n, s); }
    bool SCI_METHOD SetStyles(Sci_Position n, const char *s) override { return local.SetStyles(n, s); }
    void SCI_METHOD DecorationSetCurrentIndicator(int i) override { local.DecorationSetCurrentIndicator(i); }
    void SCI_METHOD DecorationFillRange(Sci_Position p, int v, Sci_Position n) override { local.DecorationFillRange(position(p), v, n); }
    void SCI_METHOD ChangeLexerState(Sci_Position a, Sci_Position b) override { local.ChangeLexerState(position(a), position(b)); }
    int SCI_METHOD CodePage() const override { return local.CodePage(); }
    bool SCI_METHOD IsDBCSLeadByte(char c) const override { return local.IsDBCSLeadByte(c); }
    const char *SCI_METHOD BufferPointer() override { throw Unavailable{}; }
    int SCI_METHOD GetLineIndentation(Sci_Position n) override { return local.GetLineIndentation(line(n)); }
    Sci_Position SCI_METHOD LineEnd(Sci_Position n) const override { return origin + local.LineEnd(line(n)); }
    Sci_Position SCI_METHOD GetRelativePosition(Sci_Position p, Sci_Position n) const override { auto result = local.GetRelativePosition(position(p), n); return result < 0 ? result : origin + result; }
    int SCI_METHOD GetCharacterAndWidth(Sci_Position p, Sci_Position *w) const override { return local.GetCharacterAndWidth(position(p), w); }
};
struct Session {
    std::unique_ptr<ILexer5, Release> lexer;
    std::vector<uint8_t> previous, styles;
    std::vector<int> states, levels;
    Sci_Position origin = 0, firstLine = 0, next = 0;
    bool valid = true;
};
}
extern "C" int bareline_lexilla_lex(const uint8_t *data, size_t size, const char *name, const char *keywords,
    unsigned char initialStyle, int initialState, uint32_t mode, uint8_t *styles, int32_t *states, int32_t *levels,
    size_t lineCapacity, size_t *lineCount, Cancel cancel, void *context) noexcept {
    if (!data || !name || !keywords || !styles || !states || !levels || !lineCount || size > quota || mode > 4 || lineCapacity < size + 1) return 1;
    try {
        const Lexilla::LexerModule *module = nullptr;
        for (auto candidate : {&lmCPP, &lmPython, &lmRust, &lmHTML, &lmXML, &lmCss, &lmJSON, &lmSQL, &lmTOML})
            if (std::strcmp(name, candidate->languageName) == 0) module = candidate;
        if (!module) return 2;
        Document doc(data, size, initialStyle, initialState, cancel, context);
        std::unique_ptr<ILexer5, Release> lexer(module->Create()); if (!lexer) return 3;
        lexer->PropertySet("fold", "1"); lexer->WordListSet(0, keywords);
        if (module == &lmCPP) {
            // A language profile changes upstream lexical behavior, never host state.
            // Keep C/C++ defaults; JS/TS and Go need distinct backtick semantics.
            if (mode == 1 || mode == 2 || mode == 3)
                lexer->PropertySet("lexer.cpp.enable.preprocessor", "0");
            if (mode != 0)
                lexer->PropertySet("lexer.cpp.track.preprocessor", "0");
            if (mode == 1) {
                lexer->PropertySet("lexer.cpp.backquoted.strings", "2");
                lexer->PropertySet("lexer.cpp.allow.hashes", "1");
            }
            if (mode == 2) lexer->PropertySet("lexer.cpp.backquoted.strings", "1");
            if (mode == 3 || mode == 4) lexer->PropertySet("lexer.cpp.triplequoted.strings", "1");
        }
        doc.check(); lexer->Lex(0, size, initialStyle, &doc); doc.check();
        lexer->Fold(0, size, initialStyle, &doc); doc.check();
        if (doc.stopped) return 4;
        if (doc.error) return 3;
        std::copy(doc.styles.begin(), doc.styles.end(), styles);
        std::copy(doc.states.begin(), doc.states.end(), states);
        std::copy(doc.levels.begin(), doc.levels.end(), levels);
        *lineCount = doc.states.size(); return 0;
    } catch (const Stopped &) { return 4; } catch (...) { return 3; }
}


// Exercises the complete accessor surface with boundary positions; called by
// deterministic fuzz regression tests. No user document or host pointer escapes.
extern "C" int bareline_lexilla_probe(const uint8_t *data, size_t size) noexcept {
    if (!data || size > quota) return 1;
    try {
        Document d(data, size, 7, 11, nullptr, nullptr);
        if (d.Version() != dvRelease4 || d.CodePage() != 65001 || d.IsDBCSLeadByte(char(0x81)) || d.Length() != static_cast<Sci_Position>(size)) return 2;
        d.SetErrorStatus(0);
        std::vector<char> copy(size + 1); d.GetCharRange(copy.data(), 0, 0);
        if (size) { d.GetCharRange(copy.data(), 0, size); if (std::memcmp(data, copy.data(), size)) return 3; }
        for (Sci_Position p : {Sci_Position(-1), Sci_Position(0), static_cast<Sci_Position>(size), static_cast<Sci_Position>(size + 1)}) {
            (void)d.StyleAt(p); auto line = d.LineFromPosition(p); (void)d.LineStart(line); (void)d.LineEnd(line);
            auto level = d.GetLevel(line); if (d.SetLevel(line, level) != level) return 4;
            auto state = d.GetLineState(line); if (d.SetLineState(line, state) != state) return 5;
            (void)d.GetLineIndentation(line); (void)d.GetRelativePosition(p, 1); (void)d.GetRelativePosition(p, -1);
            Sci_Position width = 0; (void)d.GetCharacterAndWidth(p, &width); if (width < 1 || width > 4) return 6;
        }
        d.StartStyling(0); if (!d.SetStyleFor(size, 3) || d.SetStyleFor(1, 3)) return 7;
        d.StartStyling(0); if (size && !d.SetStyles(size, copy.data())) return 8;
        d.DecorationSetCurrentIndicator(0); d.DecorationFillRange(0, 1, size); d.ChangeLexerState(0, size); (void)d.BufferPointer();
        bool rejected = false; char byte = 0; try { d.GetCharRange(&byte, -1, 1); } catch (const std::out_of_range &) { rejected = true; } if (!rejected) return 9;
        rejected = false; try { d.StartStyling(size + 1); } catch (const std::out_of_range &) { rejected = true; } if (!rejected) return 10;
        return 0;
    } catch (...) { return 11; }
}

extern "C" void *bareline_lexilla_session_create(const char *name, const char *keywords, uint32_t mode) noexcept {
    if (!name || !keywords || mode > 4) return nullptr;
    try {
        const Lexilla::LexerModule *module = nullptr;
        for (auto candidate : {&lmCPP, &lmPython, &lmRust, &lmHTML, &lmXML, &lmCss, &lmJSON, &lmSQL, &lmTOML})
            if (std::strcmp(name, candidate->languageName) == 0) module = candidate;
        if (!module) return nullptr;
        auto session = std::make_unique<Session>();
        session->lexer.reset(module->Create()); if (!session->lexer) return nullptr;
        auto &lexer = session->lexer;
        lexer->PropertySet("fold", "1"); lexer->WordListSet(0, keywords);
        if (module == &lmCPP) {
            if (mode == 1 || mode == 2 || mode == 3) lexer->PropertySet("lexer.cpp.enable.preprocessor", "0");
            if (mode != 0) lexer->PropertySet("lexer.cpp.track.preprocessor", "0");
            if (mode == 1) { lexer->PropertySet("lexer.cpp.backquoted.strings", "2"); lexer->PropertySet("lexer.cpp.allow.hashes", "1"); }
            if (mode == 2) lexer->PropertySet("lexer.cpp.backquoted.strings", "1");
            if (mode == 3 || mode == 4) lexer->PropertySet("lexer.cpp.triplequoted.strings", "1");
        }
        return session.release();
    } catch (...) { return nullptr; }
}
extern "C" void bareline_lexilla_session_destroy(void *handle) noexcept {
    try { delete static_cast<Session *>(handle); } catch (...) {}
}
extern "C" int bareline_lexilla_session_next(void *handle, const uint8_t *data, size_t size, size_t start,
    uint8_t *styles, int32_t *states, int32_t *levels, size_t capacity, size_t *count, Cancel cancel, void *context) noexcept {
    if (!handle || !data || !styles || !states || !levels || !count || size > quota || capacity < size + 1) return 1;
    auto &s = *static_cast<Session *>(handle);
    if (!s.valid || start != static_cast<size_t>(s.next) || start > static_cast<size_t>(PTRDIFF_MAX) - size) return 1;
    // A failed/cancelled call may have changed opaque state, so it cannot resume.
    s.valid = false;
    // Some upstream lexers retain private per-line structures. Bound their
    // lifetime as well as IDocument windows; the owner uses verified fallback
    // when this opaque-state budget is exhausted.
    if (start + size > 8 * 1024 * 1024) return 5;
    try {
        std::vector<uint8_t> combined = s.previous; combined.insert(combined.end(), data, data + size);
        AbsoluteDocument doc(combined.data(), combined.size(), s.origin, s.firstLine, cancel, context);
        std::copy(s.styles.begin(), s.styles.end(), doc.local.styles.begin());
        std::copy(s.states.begin(), s.states.end(), doc.local.states.begin());
        std::copy(s.levels.begin(), s.levels.end(), doc.local.levels.begin());
        const auto localStart = s.previous.size();
        const auto lineStart = doc.local.LineFromPosition(localStart);
        const auto style = start == 0 ? 0 : doc.StyleAt(start - 1);
        doc.local.check(); s.lexer->Lex(start, size, style, &doc); doc.local.check();
        s.lexer->Fold(start, size, style, &doc); doc.local.check();
        if (doc.local.error) return 3;
        std::copy(doc.local.styles.begin() + localStart, doc.local.styles.end(), styles);
        *count = doc.local.states.size() - lineStart;
        if (*count > capacity) return 3;
        std::copy(doc.local.states.begin() + lineStart, doc.local.states.end(), states);
        std::copy(doc.local.levels.begin() + lineStart, doc.local.levels.end(), levels);
        s.previous.assign(data, data + size);
        s.styles.assign(doc.local.styles.begin() + localStart, doc.local.styles.end());
        s.states.assign(doc.local.states.begin() + lineStart, doc.local.states.end());
        s.levels.assign(doc.local.levels.begin() + lineStart, doc.local.levels.end());
        s.origin = start; s.firstLine += lineStart; s.next += size;
        s.valid = size == 0 || data[size - 1] == '\n' || data[size - 1] == '\r';
        return 0;
    } catch (const Stopped &) { return 4; } catch (const Unavailable &) { return 5; } catch (...) { return 3; }
}

