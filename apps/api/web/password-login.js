(() => {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const status = $("status");
  let setupToken = null;
  let revision = 0;
  const changing = new URLSearchParams(location.search).get("password") === "change";
  // Static markup only: password visibility toggles and a Caps Lock hint for every password field.
  const eye = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M2.5 12s3.5-6.5 9.5-6.5 9.5 6.5 9.5 6.5-3.5 6.5-9.5 6.5S2.5 12 2.5 12Z"/><circle cx="12" cy="12" r="3"/></svg>';
  const eyeOff = '<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 4l16 16M9.9 5.8A9.9 9.9 0 0 1 12 5.5c6 0 9.5 6.5 9.5 6.5a16.6 16.6 0 0 1-3 3.7M6.6 7.1C3.9 8.9 2.5 12 2.5 12s3.5 6.5 9.5 6.5c1.6 0 3-.4 4.2-1M10 10a3 3 0 0 0 4 4"/></svg>';
  for (const input of document.querySelectorAll(".password-form input[type=\"password\"]")) {
    const field = document.createElement("div");
    field.className = "password-field";
    input.replaceWith(field);
    field.append(input);
    const toggle = document.createElement("button");
    toggle.type = "button";
    toggle.className = "password-toggle";
    toggle.setAttribute("aria-controls", input.id);
    const render = (visible) => {
      toggle.innerHTML = visible ? eyeOff : eye;
      toggle.setAttribute("aria-label", visible ? "Скрыть пароль" : "Показать пароль");
      toggle.setAttribute("aria-pressed", String(visible));
    };
    render(false);
    toggle.addEventListener("click", () => {
      const visible = input.type === "password";
      input.type = visible ? "text" : "password";
      render(visible);
      input.focus();
    });
    field.append(toggle);
    const caps = document.createElement("p");
    caps.className = "caps-hint";
    caps.hidden = true;
    caps.textContent = "Включён Caps Lock";
    field.after(caps);
    const check = (event) => { caps.hidden = !event.getModifierState?.("CapsLock"); };
    input.addEventListener("keydown", check);
    input.addEventListener("keyup", check);
    input.addEventListener("blur", () => { caps.hidden = true; });
  }
  function hideSecrets() {
    document.querySelectorAll(".password-field input").forEach((input) => { input.type = "password"; });
    document.querySelectorAll(".password-toggle").forEach((toggle) => { toggle.innerHTML = eye; toggle.setAttribute("aria-label", "Показать пароль"); toggle.setAttribute("aria-pressed", "false"); });
  }
  function show(section) {
    for (const id of ["password-sign-in", "password-setup", "password-change", "sign-in", "workspace"]) $(id).hidden = id !== section;
  }
  function csrf() {
    const name = location.protocol === "https:" ? "__Host-turnesta_csrf" : "turnesta_csrf";
    return document.cookie.split(";").map((s) => s.trim()).find((s) => s.startsWith(`${name}=`))?.slice(name.length + 1) || "";
  }
  async function request(path, body, authenticated = false) {
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), 20_000);
    try {
      const response = await fetch(path, {
        method: body ? "POST" : "GET", credentials: "same-origin", signal: controller.signal,
        headers: { ...(body ? { "Content-Type": "application/json" } : {}), ...(authenticated ? { "X-CSRF-Token": csrf() } : {}) },
        ...(body ? { body: JSON.stringify(body) } : {}),
      });
      if (!response.ok) {
        const details = await response.json().catch(() => null);
        if (path === "/auth/password/setup" && response.status === 401) {
          throw new Error("Подтверждение первого входа истекло или уже использовано. Вернитесь ко входу и попробуйте свой новый пароль или временный пароль администратора.");
        }
        const messages = {
          invalid_credentials: "Не удалось войти. Проверьте email и пароль. Для нового аккаунта используйте данные администратора.",
          invalid_password: "Используйте пароль от 15 до 128 символов без управляющих символов. Можно выбрать длинную фразу.",
          password_unchanged: "Выберите новый пароль, который отличается от прежнего.",
          password_rate_limited: "Слишком много попыток. Подождите несколько минут и попробуйте снова.",
          unauthorized: "Вход истёк. Войдите снова, чтобы изменить пароль.",
          forbidden: "Обновите страницу и повторите действие.",
        };
        throw new Error(messages[details?.code] || "Сервис временно недоступен. Попробуйте позже.");
      }
      return response.status === 204 ? null : response.json();
    } catch (error) {
      if (error instanceof TypeError || error.name === "AbortError") {
        throw new Error("Нет подтверждения от сервера. Если сохраняли новый пароль, попробуйте войти с ним. Автоматический повтор не выполняется.");
      }
      throw error;
    } finally { clearTimeout(timer); }
  }
  function lock(form, busy) {
    form.querySelectorAll("input, button").forEach((element) => { element.disabled = busy; });
  }
  function newPassword(password, confirmation) {
    if (password !== confirmation) throw new Error("Новые пароли не совпадают.");
    const length = Array.from(password).length;
    if (length < 15 || length > 128) throw new Error("Пароль должен содержать от 15 до 128 символов.");
    return password;
  }
  function entered() {
    setupToken = null;
    hideSecrets();
    document.querySelectorAll(".password-field input").forEach((input) => { input.value = ""; });
    status.textContent = "Вы вошли. Открываем кабинет…";
    location.replace("/app/");
  }
  $("password-login-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const current = ++revision;
    window.dispatchEvent(new Event("turnesta:password-auth"));
    lock(form, true);
    hideSecrets();
    status.textContent = "Входим…";
    try {
      const result = await request("/auth/password/login", { email: $("login-email").value.trim(), password: $("login-password").value });
      if (current !== revision) return;
      $("login-password").value = "";
      if (result.status === "password_change_required") {
        setupToken = result.setup_token;
        show("password-setup");
        $("page-title").textContent = "Первый вход.";
        $("page-intro").textContent = "Завершите настройку вашего аккаунта.";
        status.textContent = "Задайте свой пароль. Подтверждение первого входа действует 10 минут.";
        $("setup-title").focus();
      } else if (result.status === "authenticated") entered();
      else throw new Error("Не удалось подтвердить вход. Попробуйте снова.");
    } catch (error) { if (current === revision) status.textContent = error.message; }
    finally { lock(form, false); }
  });
  $("password-setup-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const current = ++revision;
    if (!setupToken) { status.textContent = "Введите временный пароль ещё раз на странице входа."; return; }
    lock(form, true);
    try {
      const secret = newPassword($("setup-password").value, $("setup-confirm").value);
      const result = await request("/auth/password/setup", { setup_token: setupToken, new_password: secret });
      if (current !== revision) return;
      if (result.status !== "authenticated") throw new Error("Войдите с новым паролем, чтобы проверить результат.");
      entered();
    } catch (error) { if (current === revision) status.textContent = error.message; }
    finally { lock(form, false); }
  });
  $("setup-back").addEventListener("click", () => {
    ++revision;
    setupToken = null;
    $("password-setup-form").reset();
    show("password-sign-in");
    $("page-title").textContent = "Добро пожаловать.";
    $("page-intro").textContent = "Войдите по email и паролю.";
    status.textContent = "";
    $("login-password").focus();
  });
  $("password-change-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const form = event.currentTarget;
    const current = ++revision;
    lock(form, true);
    try {
      const secret = newPassword($("change-password").value, $("change-confirm").value);
      await request("/auth/password", { current_password: $("current-password").value || null, new_password: secret }, true);
      if (current !== revision) return;
      $("login-email").value = $("account-email").value;
      form.reset();
      history.replaceState(null, "", "/login");
      show("password-sign-in");
      $("page-title").textContent = "Пароль сохранён.";
      $("page-intro").textContent = "Войдите с новым паролем. Прежние сессии завершены.";
      status.textContent = "";
      $("login-password").focus();
    } catch (error) { if (current === revision) status.textContent = error.message; }
    finally { lock(form, false); }
  });
  window.addEventListener("hashchange", () => { ++revision; setupToken = null; });
  if (changing && !new URLSearchParams(location.hash.slice(1)).get("code")) {
    show("password-change");
    $("page-title").textContent = "Доступ к кабинету.";
    $("page-intro").textContent = "Установите или измените пароль своего аккаунта.";
    lock($("password-change-form"), true);
    request("/auth/password").then((own) => {
      $("account-email").value = own.email;
      $("current-password-field").hidden = !own.requires_current_password;
      $("current-password").required = own.requires_current_password;
      lock($("password-change-form"), false);
      $(own.requires_current_password ? "current-password" : "change-password").focus();
    }).catch((error) => { status.textContent = error.message; });
  }
})();
