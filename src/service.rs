//! The server as a Windows service: `install` sets the server up and
//! registers the service, `uninstall` removes it, and `service` is what the
//! service control manager starts.
//!
//! The service keeps the host's settings as its arguments, prepares the
//! server on every start as the image does, and stops it with Ctrl+C so it
//! saves. Windows restarts it a minute after it fails.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

use windows_service::service::{
    ServiceAccess, ServiceAction, ServiceActionType, ServiceControl, ServiceControlAccept,
    ServiceErrorControl, ServiceExitCode, ServiceFailureActions, ServiceFailureResetPeriod,
    ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{
    self, ServiceControlHandlerResult, ServiceStatusHandle,
};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};
use windows_service::{define_windows_service, service_dispatcher};

use crate::settings::{Defaults, Settings};
use crate::{host, log_to_file, prepare, say};

/// The service's name.
const NAME: &str = "enshwick-server";
/// What the services list shows.
const DISPLAY_NAME: &str = "Enshrouded server (Enshwick)";
/// The firewall rule that lets players reach the server.
const FIREWALL_RULE: &str = "Enshrouded server (Enshwick)";
const TYPE: ServiceType = ServiceType::OWN_PROCESS;

/// The settings the service was started with, for the service's thread.
static SETTINGS: OnceLock<Settings> = OnceLock::new();

/// Sets the server up, opens the firewall for it, and registers and starts
/// the service, replacing one installed before.
///
/// # Errors
///
/// A step that fails; registering needs an administrator.
pub(crate) fn install(args: &[String]) -> Result<ExitCode, String> {
    let defaults = Defaults::here()?;
    let settings = Settings::read(&defaults, args, |name| std::env::var(name).ok())?;
    let layout = prepare(&settings)?;
    allow_through_firewall(&layout.server_exe())?;

    let manager = ServiceManager::local_computer(
        None::<&str>,
        ServiceManagerAccess::CONNECT | ServiceManagerAccess::CREATE_SERVICE,
    )
    .map_err(|error| format!("opening the service manager (run as administrator): {error}"))?;
    remove(&manager)?;
    let mut launch_arguments = vec![OsString::from("service")];
    launch_arguments.extend(settings.as_args(&defaults).into_iter().map(OsString::from));
    let info = ServiceInfo {
        name: OsString::from(NAME),
        display_name: OsString::from(DISPLAY_NAME),
        service_type: TYPE,
        start_type: ServiceStartType::AutoStart,
        error_control: ServiceErrorControl::Normal,
        executable_path: std::env::current_exe().map_err(|error| error.to_string())?,
        launch_arguments,
        dependencies: vec![],
        account_name: None,
        account_password: None,
    };
    let service = manager
        .create_service(
            &info,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::START | ServiceAccess::QUERY_STATUS,
        )
        .map_err(|error| format!("registering the service: {error}"))?;
    let failed = || -> windows_service::Result<()> {
        service.set_description("The Enshrouded dedicated server with the Enshwick loader")?;
        service.update_failure_actions(ServiceFailureActions {
            reset_period: ServiceFailureResetPeriod::After(Duration::from_hours(24)),
            reboot_msg: None,
            command: None,
            actions: Some(vec![
                ServiceAction {
                    action_type: ServiceActionType::Restart,
                    delay: Duration::from_secs(60),
                };
                3
            ]),
        })?;
        service.set_failure_actions_on_non_crash_failures(true)?;
        service.start::<&str>(&[])
    }();
    failed.map_err(|error| format!("configuring and starting the service: {error}"))?;
    say(&format!(
        "the service {NAME} is installed and starting; its log is {}",
        layout.log().display()
    ));
    Ok(ExitCode::SUCCESS)
}

/// Stops and removes the service and its firewall rule; the server's files
/// stay.
///
/// # Errors
///
/// The service manager cannot be opened or the service not removed.
pub(crate) fn uninstall() -> Result<ExitCode, String> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(|error| format!("opening the service manager (run as administrator): {error}"))?;
    remove(&manager)?;
    let _rule_gone = firewall(&["delete", "rule", &format!("name={FIREWALL_RULE}")]);
    say("the service is removed; the server's files stay");
    Ok(ExitCode::SUCCESS)
}

/// Stops and deletes the service when it exists.
fn remove(manager: &ServiceManager) -> Result<(), String> {
    let Ok(service) = manager.open_service(
        NAME,
        ServiceAccess::QUERY_STATUS | ServiceAccess::STOP | ServiceAccess::DELETE,
    ) else {
        return Ok(());
    };
    let status = service.query_status().map_err(|error| error.to_string())?;
    if status.current_state != ServiceState::Stopped {
        say("stopping the running service; the server saves first");
        let _stopping = service.stop();
        while service
            .query_status()
            .is_ok_and(|status| status.current_state != ServiceState::Stopped)
        {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
    service
        .delete()
        .map_err(|error| format!("removing the service: {error}"))
}

/// Allows the server's inbound traffic through Windows Firewall, replacing
/// the rule installed before.
fn allow_through_firewall(exe: &Path) -> Result<(), String> {
    let _old_gone = firewall(&["delete", "rule", &format!("name={FIREWALL_RULE}")]);
    firewall(&[
        "add",
        "rule",
        &format!("name={FIREWALL_RULE}"),
        "dir=in",
        "action=allow",
        &format!("program={}", exe.display()),
        "enable=yes",
    ])
}

fn firewall(args: &[&str]) -> Result<(), String> {
    let status = Command::new("netsh")
        .args(["advfirewall", "firewall"])
        .args(args)
        .status()
        .map_err(|error| format!("starting netsh: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "netsh advfirewall firewall {}: {status}",
            args.join(" ")
        ))
    }
}

/// Hands this process to the service control manager; returns when the
/// service stops.
///
/// # Errors
///
/// The settings do not parse, or the process was not started as a service.
pub(crate) fn dispatch(args: &[String]) -> Result<ExitCode, String> {
    let settings = Settings::read(&Defaults::here()?, args, |name| std::env::var(name).ok())?;
    log_to_file(&settings.layout().log());
    let _first = SETTINGS.set(settings);
    service_dispatcher::start(NAME, ffi_service_main)
        .map_err(|error| format!("starting as a service: {error}"))?;
    Ok(ExitCode::SUCCESS)
}

define_windows_service!(ffi_service_main, service_main);

fn service_main(_arguments: Vec<OsString>) {
    if let Err(error) = serve() {
        say(&format!("error: {error}"));
    }
}

fn serve() -> Result<(), String> {
    let settings = SETTINGS
        .get()
        .ok_or("the service started without its settings")?;
    let (stop, stopped) = mpsc::channel();
    let handler = move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _sent = stop.send(());
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    };
    let status = service_control_handler::register(NAME, handler)
        .map_err(|error| format!("registering the control handler: {error}"))?;
    report(
        status,
        ServiceState::Running,
        Duration::ZERO,
        ServiceExitCode::NO_ERROR,
    );

    let timeout = settings.stop_timeout;
    let result = prepare(settings).and_then(|layout| {
        if stopped.try_recv().is_ok() {
            return Ok(None);
        }
        let mut child = host::start(&layout)?;
        host::supervise(&mut child, &stopped, timeout, || {
            report(
                status,
                ServiceState::StopPending,
                timeout + Duration::from_secs(10),
                ServiceExitCode::NO_ERROR,
            );
        })
        .map(Some)
    });
    let exit = match &result {
        Ok(Some(exit)) if !exit.success() => ServiceExitCode::ServiceSpecific(1),
        Ok(_) => ServiceExitCode::NO_ERROR,
        Err(_) => ServiceExitCode::ServiceSpecific(2),
    };
    report(status, ServiceState::Stopped, Duration::ZERO, exit);
    result.map(|_| ())
}

fn report(
    handle: ServiceStatusHandle,
    state: ServiceState,
    wait_hint: Duration,
    exit_code: ServiceExitCode,
) {
    let controls_accepted = if state == ServiceState::Running {
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
    } else {
        ServiceControlAccept::empty()
    };
    let reported = handle.set_service_status(ServiceStatus {
        service_type: TYPE,
        current_state: state,
        controls_accepted,
        exit_code,
        checkpoint: 0,
        wait_hint,
        process_id: None,
    });
    if let Err(error) = reported {
        say(&format!("reporting the service's state: {error}"));
    }
}
