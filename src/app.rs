use std::sync::atomic::AtomicBool;
use std::thread;

use chrono::{Duration, FixedOffset};
use eframe::{egui, epi};

use crate::config::Config;
use crate::util::shared;
use crate::{
  backend::Backend,
  notifier::Notifier,
  util::{now, today, Result, Shared},
  widget,
};

pub struct App {
  scheduler_ui: widget::ScheduleUi,
  backend: Shared<dyn Backend>,
  notifier: Shared<Notifier>,
  refresh_timer: Option<thread::JoinHandle<()>>,
}

static SCROLL: AtomicBool = AtomicBool::new(true);

impl epi::App for App {
  fn name(&self) -> &str {
    "Malakal"
  }

  fn setup(
    &mut self,
    _ctx: &egui::CtxRef,
    frame: &epi::Frame,
    _storage: Option<&dyn epi::Storage>,
  ) {
    let frame = frame.clone();
    self.refresh_timer = Some(thread::spawn(move || loop {
      thread::sleep(std::time::Duration::from_millis(1000));
      frame.request_repaint();
    }));
  }

  fn update(&mut self, ctx: &egui::CtxRef, _frame: &epi::Frame) {
    self.refresh_events();
    self.load_events();

    self.scheduler_ui.update_current_time();

    egui::CentralPanel::default().show(ctx, |ui| {
      let mut scroll_area = egui::ScrollArea::both();

      if SCROLL.fetch_and(false, std::sync::atomic::Ordering::SeqCst) {
        let now = self.scheduler_ui.scroll_position_for_now();
        scroll_area = scroll_area.vertical_scroll_offset(now);
      }

      scroll_area.show(ui, |ui| self.scheduler_ui.show(ui));
    });

    self.apply_event_changes().expect("Failed applying changes");
  }
}

impl App {
  pub fn new(
    config: &Config,
    day_count: usize,
    timezone: FixedOffset,
    backend: impl Backend + 'static,
  ) -> Result<Self> {
    let first_day = today(&timezone) - Duration::days(day_count as i64 / 2);
    let backend: Shared<dyn Backend> = shared(backend);
    let notifier = shared(Notifier::start(config, &backend)?);

    let scheduler_ui = widget::ScheduleUiBuilder::default()
      .new_event_calendar(config.calendar_name.clone())
      .first_day(first_day)
      .current_time(now(&timezone))
      .timezone(timezone)
      .day_count(day_count)
      .refresh_requested(true)
      .scope_updated(true)
      .build()
      .expect("failed to build scheduler");

    Ok(Self {
      scheduler_ui,
      backend,
      notifier,
      refresh_timer: None,
    })
  }

  pub fn refresh_events(&mut self) {
    if !self.scheduler_ui.refresh_requested {
      return;
    }

    self
      .backend
      .lock()
      .unwrap()
      .force_refresh()
      .expect("failed to reload event");

    self.load_events();

    self.scheduler_ui.refresh_requested = false;
  }

  pub fn load_events(&mut self) {
    if !self.scheduler_ui.scope_updated {
      return;
    }

    let (start, end) = self.scheduler_ui.time_range();
    let events = self
      .backend
      .lock()
      .unwrap()
      .get_events(start, end)
      .expect("load events");

    self.scheduler_ui.load_events(events);
    self.scheduler_ui.scope_updated = false;
  }

  fn apply_event_changes(&mut self) -> Result<()> {
    let mut anything_changed = false;
    let mut backend = self.backend.lock().unwrap();
    let events = self.scheduler_ui.events_mut();
    for event in events.iter() {
      if event.deleted {
        backend.delete_event(&event.id)?;
        anything_changed = true;
      } else if event.changed {
        backend.update_event(event)?;
        anything_changed = true;
      }
    }

    events.retain(|e| !e.deleted);

    for event in events.iter_mut() {
      event.reset_dirty_flags();
    }

    drop(backend);

    if anything_changed {
      self.notifier.lock().unwrap().events_updated();
    }

    Ok(())
  }
}
