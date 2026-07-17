pub(crate) use craic_ui_core::ui::pages::*;
use std::rc::Rc;

pub(crate) fn build_pages(ctx: PageContext) -> Vec<PageRef> {
    vec![
        Rc::new(craic_ui_vcs::ChangesPage::new(ctx.clone())),
        Rc::new(craic_ui_vcs::HistoryPage::new(ctx.clone())),
        Rc::new(craic_ui_file::FilePage::new(ctx.clone())),
        Rc::new(craic_ui_containers::ContainersPage::new(ctx.clone())),
        Rc::new(craic_ui_agent::AgentPage::new(ctx)),
    ]
}

pub(crate) fn warm_pages_in_background(pages: &[PageRef]) {
    for page in pages {
        let label = page.label();
        log::debug!("page background initialization requested label={label}");
        page.initialize(Box::new(move |_, _| {
            log::debug!("page background initialization completed label={label}");
        }));
    }
}
