# GPUI Development Notes

Lessons learned building mp3cd-gpui, a native macOS app using Zed's GPUI framework.

## Async Progress Polling Pattern

**The Challenge:** GPUI runs on the main thread and doesn't have a built-in event system like Tauri's `window.emit()`. We needed to update the UI from a background thread during file conversion.

**What Didn't Work:**
1. Using `&mut AsyncApp` across `.await` points - lifetime errors
2. Channel-based approaches - same lifetime issues with receivers
3. Calling `entity.update()` from async context - caused "double lease" panics during rendering

**The Solution:**

```rust
fn start_progress_polling(state: ConversionState, cx: &mut Context<Self>) {
    // KEY 1: Clone AsyncApp in SYNC part, before async block
    cx.spawn(|_this: WeakEntity<Self>, cx: &mut AsyncApp| {
        let mut async_cx = cx.clone();  // Clone HERE, not inside async
        async move {
            while state.is_converting() {
                // KEY 2: Clone BEFORE each await
                let cx_for_after_await = async_cx.clone();

                Timer::after(Duration::from_millis(200)).await;

                // KEY 3: Use refresh() not entity.update() to avoid reentrancy
                let _ = cx_for_after_await.refresh();

                async_cx = cx_for_after_await;
            }
        }
    }).detach();
}
```

**Key Insights:**
- `AsyncApp` is `Clone` with `'static` lifetime (discovered in gpui source)
- `AsyncApp::update(&self)` takes `&self`, not `&mut self`
- Must clone in sync context before the `async move` block captures it
- Use `refresh()` instead of entity updates to trigger redraws without borrowing
- Share state via `Arc<Atomic*>` types that can be read without entity access

**Reentrancy Panic:**
```
cannot read FolderList while it is already being updated
```
This happens when you try to access an entity that's already borrowed (e.g., during rendering or inside an event handler). Solution: use atomic state and `refresh()` instead.

---

## Album Art Display

**Extracting Album Art:**
Uses Symphonia to read embedded artwork from audio files:

```rust
// src/audio/metadata.rs
pub fn get_album_art(path: &Path) -> Option<String> {
    // Probe the file
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &format_opts, &metadata_opts)?;

    // Check container metadata first, then format metadata
    if let Some(metadata_rev) = probed.metadata.get() {
        for visual in metadata_rev.visuals() {
            // Save to temp file and return path
            return save_album_art_to_temp(&visual.data, &visual.media_type);
        }
    }
}
```

Album art is saved to `/tmp/mp3cd_album_art/` with a hash-based filename to avoid duplicates.

**Displaying in GPUI:**

```rust
use gpui::img;
use std::path::Path;

// The img() function takes a Path, not a string
.when_some(album_art_path, |el, path| {
    el.child(
        img(Path::new(&path))
            .size_full()
            .object_fit(gpui::ObjectFit::Cover)
    )
})
```

**Important:** `img()` requires a `&Path`, not a `&str`. Convert with `Path::new(&string_path)`.

---

## Drag and Drop

**External Drops (from Finder):**

```rust
use gpui::ExternalPaths;

div()
    .on_drop(cx.listener(|this, paths: &ExternalPaths, _window, _cx| {
        this.add_external_folders(paths.paths());
    }))
    .drag_over::<ExternalPaths>(|style, _, _, _| {
        style.bg(rgb(0x3d3d3d))  // Highlight when dragging over
    })
```

**Internal Drag and Drop (reordering):**

```rust
// Create a draggable payload type
#[derive(Clone)]
pub struct DraggedFolder {
    pub index: usize,
    pub path: PathBuf,
    // ... other data for rendering drag preview
}

// Make element draggable
div()
    .on_drag(dragged_folder, |folder, _, _, cx| {
        // Return the drag preview element
        cx.new(|_| folder.clone())
    })

// Handle drops
div()
    .on_drop(cx.listener(|this, dragged: &DraggedFolder, _, _| {
        this.move_folder(dragged.index, target_index);
    }))
    .drag_over::<DraggedFolder>(|style, _, _, _| {
        style.bg(rgb(0x3d3d3d))
    })
```

---

## Theme and Appearance

**Detecting System Theme:**

```rust
impl Render for MyView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Subscribe to appearance changes (do once)
        if !self.subscribed {
            self.subscribed = true;
            cx.observe_window_appearance(window, |_this, _window, cx| {
                cx.notify();  // Trigger re-render when theme changes
            }).detach();
        }

        // Get current appearance
        let theme = Theme::from_appearance(window.appearance());

        div().bg(theme.bg)...
    }
}
```

---

## Percentage Widths

GPUI doesn't have a `pct()` function. Use `relative()` for fraction-based widths:

```rust
use gpui::relative;

// 50% width
div().w(relative(0.5))

// For progress bars:
let progress_fraction = completed as f32 / total as f32;
div().w(relative(progress_fraction))
```

---

## Common Patterns

**Conditional Rendering:**

```rust
div()
    .when(condition, |el| el.bg(color))
    .when_some(optional_value, |el, value| el.child(value))
```

**Creating Entity References:**

```rust
// Get weak reference for closures
let weak = cx.entity().downgrade();

// In closure, upgrade to use
if let Some(entity) = weak.upgrade() {
    entity.update(cx, |view, cx| { ... });
}
```

**Listener Pattern:**

```rust
// cx.listener() creates a closure that has access to self
.on_click(cx.listener(|this, _event, _window, cx| {
    this.do_something(cx);
}))
```

---

## Tokio Integration

GPUI uses `smol` internally, but for heavy async work (like parallel file conversion), spawn a separate thread with tokio:

```rust
std::thread::spawn(move || {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        // Tokio async code here
    });
});
```

The callback from tokio updates shared `Arc<Atomic*>` state, and the GPUI polling loop calls `refresh()` to trigger redraws.

---

## Cargo.toml Dependencies

```toml
[dependencies]
gpui = "0.2.2"
tokio = { version = "1", features = ["sync", "process", "rt-multi-thread", "macros"] }
futures = "0.3"
symphonia = { version = "0.5", features = ["all"] }
```

---

## Debugging Tips

1. **Entity panics:** Usually mean you're trying to borrow an entity that's already borrowed. Use atomic state and `refresh()` instead of `entity.update()`.

2. **Lifetime errors with async:** Clone `AsyncApp` in sync context before `async move`.

3. **UI not updating:** Make sure to call `cx.notify()` or `async_cx.refresh()`.

4. **Type mismatches in closures:** Check if closure expects `&mut AsyncApp` vs owned `AsyncApp`.

---

## Fixed Width vs Padding

**The Problem:** Using `.px()` (horizontal padding) makes element width variable based on content. Different button labels result in different button widths.

**The Solution:** Use `.w()` for fixed width when you want consistent sizing:

```rust
// BAD: Width varies with content
div().px(gpui::px(55.0))  // "Convert" button will be different width than "Erase" button

// GOOD: Fixed width regardless of content
div().w(gpui::px(150.0))  // All buttons same width
```

---

## Window Minimum Size

Prevent the window from being resized too small:

```rust
cx.open_window(
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(500.), px(300.))),  // Minimum dimensions
        ..Default::default()
    },
    |_window, cx| cx.new(|cx| MyView::new(cx)),
)
```

---

## CD Burning with hdiutil

**Erase + Burn in One Operation:**

Use `-erase` flag to erase a CD-RW and burn in a single operation (no eject/reinsert):

```rust
let mut args = vec!["burn", "-noverifyburn", "-puppetstrings"];
if erase_first {
    args.push("-erase");  // Erases CD-RW then burns
}
args.push(iso_path.to_str().unwrap());

Command::new("hdiutil").args(&args).spawn()
```

**Detecting CD Status with drutil:**

```rust
pub fn check_cd_status() -> Result<CdStatus, String> {
    let output = Command::new("drutil").args(["status"]).output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    if stdout.contains("No Media Inserted") {
        Ok(CdStatus::NoDisc)
    } else if stdout.contains("Overwritable: Yes") {
        // CD-RW - check if blank or has data
        if stdout.contains("Blank:                 Yes") {
            Ok(CdStatus::Blank)
        } else {
            Ok(CdStatus::ErasableWithData)
        }
    } else if stdout.contains("Blank:                 Yes") {
        Ok(CdStatus::Blank)
    } else {
        Ok(CdStatus::NonErasable)
    }
}
```

**Progress Phase Detection:**

hdiutil with `-puppetstrings` outputs progress as `PERCENT:XX`. When erasing+burning:
- Progress goes 0→100 for erase
- Then resets and goes 0→100 for burn
- Sends -1 values during internal operations (closing session, verifying)

Detect phase transitions:

```rust
let progress_callback = Box::new(move |progress: i32| {
    let prev = last_progress.load(Ordering::SeqCst);

    // -1 after ≥95% means "Finishing" stage
    if progress < 0 && prev >= 95 {
        state.set_stage(BurnStage::Finishing);
        return;
    }

    // High→low transition means erase finished, burn starting
    if prev > 50 && progress < 20 {
        state.set_stage(BurnStage::Burning);
    }

    last_progress.store(progress, Ordering::SeqCst);
    state.set_burn_progress(progress);
});
```

---

## Hidden UI Features

For a cleaner UI, make elements clickable without explicit buttons:

```rust
// Progress display doubles as cancel button (hidden feature)
div()
    .id(SharedString::from("progress-display"))
    .when(is_cancelable, |el| {
        el.cursor_pointer()  // Subtle hint
            .on_click(cx.listener(|this, _event, _window, _cx| {
                this.conversion_state.request_cancel();
            }))
    })
```

The cursor change to pointer provides a subtle hint without adding visual clutter.
