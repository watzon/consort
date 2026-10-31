//! The limits of a worker in Settings > Workers. Each change goes to the worker
//! at once. The worker keeps the limits, so it must be online.

use consort_client::ApprovalMode;
use consort_theme::Tokens;
use gpui_kit::{
    Context, Div, Focusable as _, InteractiveElement as _, ParentElement as _, Pixels,
    StatefulInteractiveElement as _, Styled as _, Window, div,
};

use crate::app::{ConsortApp, DialogConfirm};
use crate::ui::dialogs::{field_block, input_box};
use crate::ui::parts::{ButtonKind, button, detail, selectable, text};

/// A list of folders with `folder` added. Returns `None` when the list has it.
pub(crate) fn with_folder(folders: &[String], folder: &str) -> Option<Vec<String>> {
    if folders.iter().any(|f| f == folder) {
        return None;
    }
    let mut list = folders.to_vec();
    list.push(folder.to_owned());
    Some(list)
}

/// A list of folders with the folder at `at` taken out.
pub(crate) fn without_folder(folders: &[String], at: usize) -> Vec<String> {
    let mut list = folders.to_vec();
    if at < list.len() {
        list.remove(at);
    }
    list
}

impl ConsortApp {
    /// Sends a new highest approval mode or a new shell access to the worker.
    pub(crate) fn set_worker_limit(
        &mut self,
        worker_id: String,
        mode: Option<ApprovalMode>,
        shell: Option<bool>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let client = self.client.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client.set_worker_policy(worker_id, mode, None, shell).await;
            if let Err(err) = result {
                let _ = this.update_in(cx, |this, _, cx| this.show_error(err.to_string(), cx));
            }
        })
        .detach();
    }

    /// Sends a new list of folders to the worker. An error shows in the dialog.
    /// The typed folder stays in the field until the worker takes it.
    fn save_worker_folders(
        &mut self,
        worker_id: String,
        folders: Vec<String>,
        clear_entry: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dialog_error = None;
        let client = self.client.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = client
                .set_worker_policy(worker_id, None, Some(folders), None)
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) if clear_entry => this
                        .inputs
                        .folder
                        .update(cx, |input, cx| input.set_value("", window, cx)),
                    Ok(()) => {}
                    Err(err) => this.dialog_error = Some(err.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_worker_folder(
        &mut self,
        worker_id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = self.field_text(&self.inputs.folder, cx);
        if folder.is_empty() {
            self.dialog_error = Some("Enter the full path of a folder on this worker.".into());
            cx.notify();
            return;
        }
        let current = self
            .state
            .worker(&worker_id)
            .map(|w| w.policy.allowed_roots.clone())
            .unwrap_or_default();
        match with_folder(&current, &folder) {
            Some(folders) => self.save_worker_folders(worker_id, folders, true, window, cx),
            None => {
                self.inputs
                    .folder
                    .update(cx, |input, cx| input.set_value("", window, cx));
                cx.notify();
            }
        }
    }

    /// The Edit folders dialog. It has a text field and no folder picker,
    /// because the folder may be on another machine.
    pub(crate) fn worker_folders_dialog(
        &mut self,
        id: String,
        t: &Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (String, String, Pixels, Div, Div) {
        let c = &t.colors;
        let (name, folders) = self
            .state
            .worker(&id)
            .map(|w| (w.name.clone(), w.policy.allowed_roots.clone()))
            .unwrap_or_default();
        let mut list = div().flex().flex_col().gap(t.space.xs);
        if folders.is_empty() {
            list = list.child(
                text(
                    "No limit. Tasks can use any folder on this worker.",
                    detail(t),
                    c.text_tertiary,
                )
                .debug_selector(|| "folders-no-limit".into()),
            );
        }
        for (at, folder) in folders.iter().enumerate() {
            let worker = id.clone();
            let all = folders.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .debug_selector(|| format!("folder-row-{at}"))
                    .child(div().flex_1().min_w_0().child(selectable(
                        format!("folder-{at}"),
                        folder.clone(),
                        detail(t),
                        c.text_primary,
                    )))
                    .child(
                        button(
                            format!("folder-remove-{at}"),
                            "Remove",
                            ButtonKind::Quiet,
                            t,
                        )
                        .debug_selector(|| format!("folder-remove-{at}"))
                        .flex_none()
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.save_worker_folders(
                                    worker.clone(),
                                    without_folder(&all, at),
                                    false,
                                    window,
                                    cx,
                                );
                            },
                        )),
                    ),
            );
        }
        let focused = self
            .inputs
            .folder
            .read(cx)
            .focus_handle(cx)
            .is_focused(window);
        let add_id = id.clone();
        // Enter in the field adds the folder.
        let enter_id = id.clone();
        let on_enter: DialogConfirm = std::rc::Rc::new(move |this, window, cx| {
            this.add_worker_folder(enter_id.clone(), window, cx);
        });
        self.dialog_confirm.replace(Some(on_enter));
        let body = div()
            .flex()
            .flex_col()
            .gap(t.space.md)
            .child(list)
            .child(field_block(
                "Add a folder",
                div()
                    .flex()
                    .items_center()
                    .gap(t.space.sm)
                    .child(div().flex_1().min_w_0().child(input_box(
                        &self.inputs.folder,
                        focused,
                        t,
                    )))
                    .child(
                        button("folder-add", "Add folder", ButtonKind::Secondary, t)
                            .debug_selector(|| "folder-add".into())
                            .flex_none()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_worker_folder(add_id.clone(), window, cx);
                            })),
                    ),
                Some("Enter the full path, as the worker writes it."),
                t,
            ));
        let actions = div()
            .flex()
            .items_center()
            .justify_end()
            .pt(t.space.lg)
            .border_t(t.border_width)
            .border_color(c.border_subtle)
            .child(
                button("dialog-confirm", "Done", ButtonKind::Primary, t)
                    .debug_selector(|| "dialog-confirm".into())
                    .on_click(cx.listener(|this, _, window, cx| this.close_dialog(window, cx))),
            );
        (
            "Allowed folders".to_owned(),
            format!("Tasks on {name} can use only these folders. Each change applies at once."),
            t.layout.dialog_width,
            body,
            actions,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_goes_in_the_list_once_and_comes_out_by_its_place() {
        let list = vec!["/a".to_owned(), "/b".to_owned()];
        assert_eq!(with_folder(&list, "/a"), None);
        assert_eq!(with_folder(&list, "/c").unwrap().len(), 3);
        assert_eq!(without_folder(&list, 0), vec!["/b".to_owned()]);
        assert_eq!(without_folder(&list, 9), list);
    }
}
