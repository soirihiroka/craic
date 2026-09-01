impl DiffCanvas {
    pub fn new() -> Self {
        let area = skia_gl_area::new_area();
        area.set_size_request(MIN_CONTENT_WIDTH, 160);
        let spinner = adw::Spinner::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .build();
        spinner.set_size_request(32, 32);
        let root = gtk::Overlay::builder().hexpand(true).vexpand(true).build();
        root.set_child(Some(&area));
        root.add_overlay(&spinner);

        let font_size = config::load().font_sizes.diff;
        let state = Rc::new(DiffCanvasState {
            rows: RefCell::new(Vec::new()),
            font_size: Cell::new(font_size),
            scroll_y: Cell::new(0.0),
            content_height: Cell::new(1.0),
            overshoot: canvas_overshoot::EdgeGlow::new(),
            scrollbar_hover: Rc::new(Cell::new(false)),
            scrollbar_active: Rc::new(Cell::new(false)),
            scrollbar_hover_progress: Rc::new(Cell::new(0.0)),
            scrollbar_animating: Rc::new(Cell::new(false)),
            scrollbar_smooth_scroll: canvas_scrollbar::SmoothScroll::new(),
            scrollbar_drag: Cell::new(None),
            middle_autoscroll: Rc::new(canvas_scroll::MiddleAutoscroll::new()),
            fold_callback: RefCell::new(None),
            layout_generation: Cell::new(1),
            layout_cache: RefCell::new(None),
            layout_pending_signature: RefCell::new(None),
            layout_request_id: Cell::new(0),
            text_width_cache: RefCell::new(canvas::TextWidthCache::new(font_size)),
            max_line_number: Cell::new(1),
            fold_row_count: Cell::new(0),
            syntax: RefCell::new(Vec::new()),
            syntax_signature: RefCell::new(None),
            selection: RefCell::new(None),
            selection_drag: Cell::new(None),
            active_side: Cell::new(DiffCanvasSide::Right),
            search: RefCell::new(DiffSearchState::default()),
        });

        skia_gl_area::install(&area, {
            let state = state.clone();
            move |area, context, width, height| draw(area, context, width, height, &state)
        });
        area.connect_resize({
            let state = state.clone();
            let spinner = spinner.clone();
            move |area, _, _| {
                request_layout(area, &state, &spinner);
                clamp_scroll(area, &state);
            }
        });

        install_scroll(&area, &state);
        install_diff_middle_autoscroll(&area, &state);
        install_clicks(&area, &state, &spinner);
        install_motion(&area, &state);
        install_key_shortcuts(&area, &state, &spinner);

        Self {
            root,
            area,
            spinner,
            state,
        }
    }

    pub fn set_rows(&self, rows: Vec<DiffRow>) {
        let fold_rows = rows.iter().filter(|row| is_fold_row(row)).count();
        let max_line_number = rows
            .iter()
            .flat_map(|row| [row.left_number, row.right_number])
            .flatten()
            .max()
            .unwrap_or(1);
        if diff_rows_equal(&self.state.rows.borrow(), &rows) {
            self.state.fold_row_count.set(fold_rows);
            self.state.max_line_number.set(max_line_number);
            return;
        }

        stop_diff_middle_autoscroll(&self.area, &self.state);
        self.state.fold_row_count.set(fold_rows);
        self.state.max_line_number.set(max_line_number);
        self.state.rows.replace(rows);
        self.state.selection.borrow_mut().take();
        self.state.selection_drag.set(None);
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.layout_pending_signature.borrow_mut().take();
        rebuild_search_matches(&self.area, &self.state);
        request_layout(&self.area, &self.state, &self.spinner);
        clamp_scroll(&self.area, &self.state);
        self.area.queue_render();
    }

    pub fn set_syntax_for_file(&self, file_path: &str, fingerprint: u64, full_rows: &[DiffRow]) {
        update_syntax_state(&self.state, file_path, fingerprint, full_rows);
    }

    pub fn clear(&self) {
        stop_diff_middle_autoscroll(&self.area, &self.state);
        self.state.rows.borrow_mut().clear();
        self.state.scroll_y.set(0.0);
        self.state.max_line_number.set(1);
        self.state.fold_row_count.set(0);
        self.state.syntax.borrow_mut().clear();
        self.state.syntax_signature.borrow_mut().take();
        self.state.selection.borrow_mut().take();
        self.state.selection_drag.set(None);
        self.state.search.borrow_mut().matches.clear();
        self.state.search.borrow_mut().active = None;
        self.state
            .layout_generation
            .set(self.state.layout_generation.get().wrapping_add(1).max(1));
        self.state.layout_cache.borrow_mut().take();
        self.state.layout_pending_signature.borrow_mut().take();
        self.state.content_height.set(1.0);
        self.spinner.set_visible(false);
        self.area.queue_render();
    }

    pub fn scroll_y(&self) -> f64 {
        self.state.scroll_y.get()
    }

    pub fn set_scroll_y(&self, scroll_y: f64) {
        if self.state.layout_cache.borrow().is_some() {
            set_scroll_y(&self.area, &self.state, scroll_y);
        } else {
            // A replacement diff is laid out off-thread. Keep the requested
            // position until that layout arrives, then clamp it to its bounds.
            self.state.scroll_y.set(scroll_y.max(0.0));
            self.area.queue_render();
        }
    }

    pub fn set_fold_callback<F>(&self, callback: F)
    where
        F: Fn(usize) + 'static,
    {
        self.state.fold_callback.replace(Some(Rc::new(callback)));
    }

    pub fn focus(&self) {
        self.area.grab_focus();
    }

    pub fn set_search_query(&self, query: &str) {
        let changed = {
            let mut search = self.state.search.borrow_mut();
            if search.query == query {
                false
            } else {
                search.query = query.to_string();
                true
            }
        };
        if changed {
            rebuild_search_matches(&self.area, &self.state);
        } else {
            select_active_search_match(&self.area, &self.state);
        }
        self.area.queue_render();
    }

    pub fn search_next(&self) {
        let len = self.state.search.borrow().matches.len();
        if len == 0 {
            return;
        }
        {
            let mut search = self.state.search.borrow_mut();
            search.active = Some(search.active.map(|active| (active + 1) % len).unwrap_or(0));
        }
        select_active_search_match(&self.area, &self.state);
        self.area.queue_render();
    }

    pub fn search_previous(&self) {
        let len = self.state.search.borrow().matches.len();
        if len == 0 {
            return;
        }
        {
            let mut search = self.state.search.borrow_mut();
            search.active = Some(
                search
                    .active
                    .map(|active| active.checked_sub(1).unwrap_or(len - 1))
                    .unwrap_or(len - 1),
            );
        }
        select_active_search_match(&self.area, &self.state);
        self.area.queue_render();
    }

    pub fn search_status(&self) -> String {
        let search = self.state.search.borrow();
        if search.query.is_empty() {
            return String::new();
        }
        let Some(active) = search.active else {
            return "No Results".to_string();
        };
        format!("{} of {}", active + 1, search.matches.len())
    }
}
