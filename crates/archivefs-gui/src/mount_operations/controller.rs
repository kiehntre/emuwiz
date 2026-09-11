use crate::*;

impl ArchiveFsApp {
    pub(crate) fn start_mount_all(&mut self, context: egui::Context, items: Vec<MountAllItem>) -> bool {
        if self.is_busy() {
            let message = "Another archive operation is already running.".to_string();
            self.feedback = Some(ActionFeedback {
                succeeded: false,
                message: message.clone(),
                cleanup: None,
                warning: None,
                more_information: None,
            });
            self.history.record(HistoryEntry::new(
                ActivityAction::MountAll,
                None,
                ActivityOutcome::Rejected,
                message,
            ));
            return false;
        }

        let total = items.len();
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        self.confirm_mount_all = None;
        self.confirm_unmount_all = None;
        self.confirm_unmount_selected = None;
        self.focus_mount_all_cancel = false;
        self.confirm_unmount = None;
        self.confirm_lazy_unmount = None;
        self.confirm_lazy_unmount_final = None;
        self.mount_all_result = None;
        self.unmount_all_result = None;
        self.feedback = None;
        self.history.record(HistoryEntry::new(
            ActivityAction::MountAll,
            None,
            ActivityOutcome::Started,
            format!("Mount All started for {total} pending archives."),
        ));
        self.mount_all = Some(RunningMountAll {
            receiver,
            stop,
            progress: MountAllProgress {
                total,
                ..MountAllProgress::default()
            },
        });

        thread::spawn(move || {
            let archive_paths = items
                .iter()
                .map(|item| item.archive_path.clone())
                .collect::<Vec<_>>();
            let setup = Config::load_default()
                .and_then(|config| ArchiveMountSession::new(&config))
                .map_err(|error| error.to_string())
                .and_then(|session| {
                    session
                        .validate_batch_targets(&archive_paths)
                        .map_err(|error| error.to_string())
                        .map(|validations| {
                            let validations = validations
                                .into_iter()
                                .map(|validation| {
                                    (validation.archive_path().to_path_buf(), validation)
                                })
                                .collect::<HashMap<_, _>>();
                            (session, validations)
                        })
                });
            let (session, validations) = match setup {
                Ok(setup) => setup,
                Err(error) => {
                    let _ = sender.send(MountAllEvent::Finished(MountAllResult::setup_failed(
                        total, error,
                    )));
                    context.request_repaint();
                    return;
                }
            };
            let repaint_context = context.clone();
            run_mount_all_coordinator(
                items,
                &worker_stop,
                |archive_path| archive_path.is_file(),
                |archive_path| match validations.get(archive_path) {
                    Some(validation) => validation
                        .skip_reason()
                        .map_or(Ok(()), |reason| Err(reason.to_string())),
                    None => Err("archive was not included in batch validation".to_string()),
                },
                |archive_path| {
                    let validation = validations.get(archive_path).ok_or_else(|| {
                        "archive was not included in batch validation".to_string()
                    })?;
                    match session
                        .mount_validated_batch_target(validation)
                        .map_err(|error| error.to_string())?
                    {
                        MountOneOutcome::Mounted(plan) => {
                            Ok(BatchMountAttempt::Mounted(plan.mount_path))
                        }
                        MountOneOutcome::AlreadyMounted(plan) => {
                            Ok(BatchMountAttempt::AlreadyMounted(plan.mount_path))
                        }
                    }
                },
                |event| {
                    let _ = sender.send(event);
                    repaint_context.request_repaint();
                },
            );
        });
        true
    }

    pub(crate) fn request_mount_all_stop(&mut self) {
        let Some(batch) = self.mount_all.as_mut() else {
            return;
        };
        if batch.progress.stop_requested {
            return;
        }
        batch.progress.stop_requested = true;
        batch.stop.store(true, Ordering::Release);
        self.history.record(HistoryEntry::new(
            ActivityAction::MountAll,
            None,
            ActivityOutcome::Cancelled,
            "Stop requested; the current archive will finish before Mount All stops.",
        ));
    }

    pub(crate) fn poll_mount_all(&mut self, context: &egui::Context) {
        let mut disconnected = false;
        let mut events = Vec::new();
        if let Some(batch) = self.mount_all.as_ref() {
            loop {
                match batch.receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }

        let mut finished = None;
        for event in events {
            let Some(batch) = self.mount_all.as_mut() else {
                break;
            };
            match event {
                MountAllEvent::ArchiveStarted { index, total, item } => {
                    batch.progress.current_index = index;
                    batch.progress.total = total;
                    batch.progress.current_archive = Some(item.display_name.clone());
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Mount,
                        Some(item.archive_path),
                        ActivityOutcome::Started,
                        format!("Mounting archive {index} of {total}."),
                    ));
                }
                MountAllEvent::ArchiveCompleted(item) => {
                    batch.progress.successful += 1;
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Mount,
                        Some(item.archive_path),
                        ActivityOutcome::Completed,
                        format!("Mounted at {}.", item.mount_path.display()),
                    ));
                }
                MountAllEvent::ArchiveFailed { item, message } => {
                    batch.progress.failed += 1;
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Mount,
                        Some(item.archive_path),
                        ActivityOutcome::Failed,
                        message,
                    ));
                }
                MountAllEvent::ArchiveSkipped { item, reason } => {
                    batch.progress.skipped += 1;
                    batch.progress.current_index =
                        batch.progress.successful + batch.progress.failed + batch.progress.skipped;
                    batch.progress.current_archive = Some(item.display_name.clone());
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Mount,
                        Some(item.archive_path),
                        ActivityOutcome::Skipped,
                        reason,
                    ));
                }
                MountAllEvent::Finished(result) => {
                    finished = Some(result);
                }
            }
        }

        if let Some(result) = finished {
            let message = result.completion_message();
            let setup_failure = result.setup_failure.as_deref();
            let activity_message = match setup_failure {
                Some(error) => format!("{message} Setup error: {error}"),
                None => format!(
                    "{message} Successful: {}, failed: {}, skipped: {}, unattempted: {}.",
                    result.successful,
                    result.failed(),
                    result.skipped(),
                    result.unattempted
                ),
            };
            self.history.record(HistoryEntry::new(
                ActivityAction::MountAll,
                None,
                if setup_failure.is_some() {
                    ActivityOutcome::Failed
                } else {
                    ActivityOutcome::Completed
                },
                activity_message,
            ));
            self.feedback = Some(ActionFeedback {
                succeeded: setup_failure.is_none(),
                message: match setup_failure {
                    Some(error) => format!("{message} {error}"),
                    None => message.clone(),
                },
                cleanup: None,
                warning: None,
                more_information: None,
            });
            let should_refresh = result.setup_failure.is_none();
            self.mount_all_result = Some(result);
            self.mount_all = None;
            if should_refresh {
                self.refresh(context);
            }
        } else if disconnected && self.mount_all.is_some() {
            let message = "Mount All background worker stopped unexpectedly.".to_string();
            self.history.record(HistoryEntry::new(
                ActivityAction::MountAll,
                None,
                ActivityOutcome::Failed,
                message.clone(),
            ));
            self.feedback = Some(ActionFeedback {
                succeeded: false,
                message,
                cleanup: None,
                warning: None,
                more_information: None,
            });
            self.mount_all = None;
            self.refresh(context);
        }
    }

    pub(crate) fn start_unmount_all(
        &mut self,
        context: egui::Context,
        items: Vec<UnmountAllItem>,
        cleanup_after_unmount: bool,
    ) -> bool {
        if self.is_busy() {
            let message = "Another archive operation is already running.".to_string();
            self.feedback = Some(ActionFeedback {
                succeeded: false,
                message: message.clone(),
                cleanup: None,
                warning: None,
                more_information: None,
            });
            self.history.record(HistoryEntry::new(
                ActivityAction::UnmountAll,
                None,
                ActivityOutcome::Rejected,
                message,
            ));
            return false;
        }

        let total = items.len();
        let (sender, receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        self.confirm_mount_all = None;
        self.confirm_unmount_all = None;
        self.confirm_unmount_selected = None;
        self.confirm_unmount = None;
        self.confirm_lazy_unmount = None;
        self.confirm_lazy_unmount_final = None;
        self.unmount_all_result = None;
        self.mount_all_result = None;
        self.feedback = None;
        self.history.record(HistoryEntry::new(
            ActivityAction::UnmountAll,
            None,
            ActivityOutcome::Started,
            format!("Unmount All started for {total} mounted archives."),
        ));
        self.unmount_all = Some(RunningUnmountAll {
            receiver,
            stop,
            progress: UnmountAllProgress {
                total,
                ..Default::default()
            },
        });

        thread::spawn(move || {
            let setup = Config::load_default()
                .and_then(|config| {
                    ArchiveUnmountSession::new(&config).map(|session| (config, session))
                })
                .map_err(|error| error.to_string());
            let (config, session) = match setup {
                Ok(setup) => setup,
                Err(error) => {
                    let _ = sender.send(UnmountAllEvent::Finished(UnmountAllResult::setup_failed(
                        total, error,
                    )));
                    context.request_repaint();
                    return;
                }
            };
            let repaint_context = context.clone();
            run_unmount_all_coordinator(
                items,
                &worker_stop,
                |item| match session
                    .unmount_archive_path(&item.archive_path, &item.mount_path)
                    .map_err(|error| BatchUnmountError {
                        offer_lazy_unmount: error.allows_lazy_unmount_recovery(),
                        message: error.to_string(),
                    })? {
                    UnmountOneOutcome::NotMounted(_) => Ok(BatchUnmountAttempt::NotMounted),
                    UnmountOneOutcome::Unmounted(_) => Ok(BatchUnmountAttempt::Unmounted),
                },
                |item, publish| {
                    cleanup_after_unmount.then(|| {
                        publish(UnmountAllEvent::CleanupStarted(item.mount_path.clone()));
                        cleanup_selected_mount_tree(&config, &item.mount_path)
                            .map(|_| ())
                            .map_err(|error| error.to_string())
                    })
                },
                |event| {
                    let _ = sender.send(event);
                    repaint_context.request_repaint();
                },
            );
        });
        true
    }

    pub(crate) fn request_unmount_all_stop(&mut self) {
        let Some(batch) = self.unmount_all.as_mut() else {
            return;
        };
        if batch.progress.stop_requested {
            return;
        }
        batch.progress.stop_requested = true;
        batch.stop.store(true, Ordering::Release);
        self.history.record(HistoryEntry::new(
            ActivityAction::UnmountAll,
            None,
            ActivityOutcome::Cancelled,
            "Stop requested; the current archive will finish before Unmount All stops.",
        ));
    }

    pub(crate) fn poll_unmount_all(&mut self, context: &egui::Context) {
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(batch) = self.unmount_all.as_ref() {
            loop {
                match batch.receiver.try_recv() {
                    Ok(event) => events.push(event),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        let mut finished = None;
        for event in events {
            let Some(batch) = self.unmount_all.as_mut() else {
                break;
            };
            match event {
                UnmountAllEvent::ArchiveStarted { index, total, item } => {
                    batch.progress.current_index = index;
                    batch.progress.total = total;
                    batch.progress.current_archive = Some(item.display_name.clone());
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Unmount,
                        Some(item.archive_path),
                        ActivityOutcome::Started,
                        format!("Unmounting archive {index} of {total}."),
                    ));
                }
                UnmountAllEvent::ArchiveCompleted(item) => {
                    batch.progress.successful += 1;
                    set_lazy_unmount_offer(
                        &mut self.lazy_unmount_offers,
                        &item.archive_path,
                        false,
                    );
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Unmount,
                        Some(item.archive_path),
                        ActivityOutcome::Completed,
                        format!("Unmounted {}.", item.mount_path.display()),
                    ));
                }
                UnmountAllEvent::ArchiveFailed {
                    item,
                    message,
                    offer_lazy_unmount,
                } => {
                    batch.progress.failed += 1;
                    if offer_lazy_unmount {
                        set_lazy_unmount_offer(
                            &mut self.lazy_unmount_offers,
                            &item.archive_path,
                            true,
                        );
                        self.history.record(HistoryEntry::new(
                            ActivityAction::LazyUnmount,
                            Some(item.archive_path.clone()),
                            ActivityOutcome::Offered,
                            "Lazy unmount offered for individual recovery after normal unmount failed.",
                        ));
                    }
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Unmount,
                        Some(item.archive_path),
                        ActivityOutcome::Failed,
                        format!("Normal unmount failed: {message}"),
                    ));
                }
                UnmountAllEvent::ArchiveSkipped { item, reason } => {
                    batch.progress.skipped += 1;
                    set_lazy_unmount_offer(
                        &mut self.lazy_unmount_offers,
                        &item.archive_path,
                        false,
                    );
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Unmount,
                        Some(item.archive_path),
                        ActivityOutcome::Skipped,
                        reason,
                    ));
                }
                UnmountAllEvent::CleanupStarted(path) => {
                    record_cleanup_started_activity(&mut self.history, &path);
                }
                UnmountAllEvent::CleanupCompleted(path) => {
                    batch.progress.cleanup_successes += 1;
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Cleanup,
                        Some(path.clone()),
                        ActivityOutcome::Completed,
                        format!("Cleanup completed for {}.", path.display()),
                    ));
                }
                UnmountAllEvent::CleanupFailed {
                    mount_path,
                    message,
                } => {
                    batch.progress.cleanup_failures += 1;
                    self.history.record(HistoryEntry::new(
                        ActivityAction::Cleanup,
                        Some(mount_path),
                        ActivityOutcome::Failed,
                        message,
                    ));
                }
                UnmountAllEvent::Finished(result) => finished = Some(result),
            }
        }
        if let Some(result) = finished {
            let message = result.completion_message();
            let setup_failed = result.setup_failure.is_some();
            let setup_failure = result.setup_failure.as_deref();
            self.history.record(HistoryEntry::new(
                ActivityAction::UnmountAll,
                None,
                if setup_failed {
                    ActivityOutcome::Failed
                } else {
                    ActivityOutcome::Completed
                },
                setup_failure.map_or_else(
                    || message.clone(),
                    |error| format!("{message} Setup error: {error}"),
                ),
            ));
            self.feedback = Some(ActionFeedback {
                succeeded: !setup_failed,
                message: setup_failure
                    .map_or_else(|| message.clone(), |error| format!("{message} {error}")),
                cleanup: None,
                warning: None,
                more_information: None,
            });
            self.unmount_all_result = Some(result);
            self.unmount_all = None;
            if !setup_failed {
                self.refresh(context);
            }
        } else if disconnected && self.unmount_all.is_some() {
            let message = "Unmount All background worker stopped unexpectedly.".to_string();
            self.history.record(HistoryEntry::new(
                ActivityAction::UnmountAll,
                None,
                ActivityOutcome::Failed,
                message.clone(),
            ));
            self.feedback = Some(ActionFeedback {
                succeeded: false,
                message,
                cleanup: None,
                warning: None,
                more_information: None,
            });
            self.unmount_all = None;
            self.refresh(context);
        }
    }


}
