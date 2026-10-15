//! Desktop Windows oculto (`CreateDesktopW`): o Edge roda com janela normal, mas fora da tela
//! do usuário e sem roubar o foco. Porte do `edge_isolado.py` da skill `nodriver-isolado`.
//!
//! Processos criados com `STARTUPINFOW.lpDesktop = "WinSta0\<nome>"` abrem as janelas nesse
//! desktop; os filhos (o Edge se relança) herdam o mesmo desktop.

#[cfg(not(windows))]
use std::io;
use std::path::Path;

/// Junta argumentos numa linha de comando do Windows (regras do MSVCRT, como `list2cmdline`).
pub fn linha_de_comando(exe: &Path, args: &[String]) -> String {
    let mut partes = vec![citar(&exe.display().to_string())];
    partes.extend(args.iter().map(|a| citar(a)));
    partes.join(" ")
}

fn citar(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    let mut s = String::from("\"");
    let mut barras = 0usize;
    for c in arg.chars() {
        match c {
            '\\' => barras += 1,
            '"' => {
                s.push_str(&"\\".repeat(barras * 2 + 1));
                s.push('"');
                barras = 0;
            }
            _ => {
                s.push_str(&"\\".repeat(barras));
                barras = 0;
                s.push(c);
            }
        }
    }
    s.push_str(&"\\".repeat(barras * 2));
    s.push('"');
    s
}

#[cfg(windows)]
mod imp {
    use std::io;
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL};
    use windows_sys::Win32::System::StationsAndDesktops::{CloseDesktop, CreateDesktopW, HDESK};
    use windows_sys::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION, STARTUPINFOW};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// Desktop oculto; fecha no `Drop`.
    pub struct DesktopOculto {
        nome: String,
        handle: HDESK,
    }

    // O handle é só passado de volta para CloseDesktop.
    unsafe impl Send for DesktopOculto {}
    unsafe impl Sync for DesktopOculto {}

    impl DesktopOculto {
        pub fn criar() -> io::Result<Self> {
            let mut bytes = [0u8; 4];
            getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
            let nome = format!("nd-oculto-{:08x}", u32::from_le_bytes(bytes));
            let w = wide(&nome);
            let handle = unsafe {
                CreateDesktopW(
                    w.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                    GENERIC_ALL,
                    std::ptr::null(),
                )
            };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { nome, handle })
        }

        pub fn nome(&self) -> &str {
            &self.nome
        }
    }

    impl Drop for DesktopOculto {
        fn drop(&mut self) {
            unsafe {
                CloseDesktop(self.handle);
            }
        }
    }

    /// Cria um processo com as janelas no desktop `nome`. Devolve o pid.
    pub fn criar_processo(nome: &str, exe: &Path, args: &[String]) -> io::Result<u32> {
        let mut linha = wide(&super::linha_de_comando(exe, args));
        let mut desktop = wide(&format!("WinSta0\\{nome}"));
        let mut si: STARTUPINFOW = unsafe { std::mem::zeroed() };
        si.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        si.lpDesktop = desktop.as_mut_ptr();
        let mut pi: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        let ok = unsafe {
            CreateProcessW(
                std::ptr::null(),
                linha.as_mut_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                0,
                std::ptr::null(),
                std::ptr::null(),
                &si,
                &mut pi,
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        unsafe {
            CloseHandle(pi.hThread);
            CloseHandle(pi.hProcess);
        }
        Ok(pi.dwProcessId)
    }
}

#[cfg(windows)]
pub use imp::{DesktopOculto, criar_processo};

#[cfg(not(windows))]
pub struct DesktopOculto;

#[cfg(not(windows))]
impl DesktopOculto {
    pub fn criar() -> io::Result<Self> {
        Err(io::Error::other("desktop oculto só existe no Windows"))
    }
    pub fn nome(&self) -> &str {
        ""
    }
}

#[cfg(not(windows))]
pub fn criar_processo(_nome: &str, _exe: &Path, _args: &[String]) -> io::Result<u32> {
    Err(io::Error::other("desktop oculto só existe no Windows"))
}

#[cfg(test)]
mod testes {
    use super::*;

    #[test]
    fn citacao() {
        assert_eq!(citar("--no-first-run"), "--no-first-run");
        assert_eq!(
            citar(r"--user-data-dir=C:\a b\c"),
            r#""--user-data-dir=C:\a b\c""#
        );
        assert_eq!(citar(r"C:\x\"), r"C:\x\");
        assert_eq!(citar(r"a b\"), r#""a b\\""#);
        assert_eq!(citar(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(citar(""), r#""""#);
    }
}
