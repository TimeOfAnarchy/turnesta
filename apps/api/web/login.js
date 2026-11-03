(() => {
"use strict";

const form = document.querySelector("#login-form");
const input = document.querySelector("#code");
const status = document.querySelector("#status");
const button = document.querySelector("#login-button");
const workspace = document.querySelector("#workspace");
const signIn = document.querySelector("#sign-in");
const changingPassword = new URLSearchParams(location.search).get("password") === "change";
const list = document.querySelector("#organizations");
const moreButton = document.querySelector("#more-button");
let nextCursor = null;
let viewRevision = 0;
window.addEventListener("turnesta:password-auth", () => { viewRevision += 1; });

function takeInvitation() {
  const fragment = new URLSearchParams(location.hash.slice(1));
  const invitation = fragment.get("code");
  if (location.hash) history.replaceState(null, "", location.pathname);
  if (!invitation || !/^[0-9a-f]{64}$/.test(invitation)) return false;
  input.value = invitation;
  document.querySelector("#password-sign-in").hidden = true;
  document.querySelector("#password-change").hidden = true;
  document.querySelector("#password-setup").hidden = true;
  signIn.hidden = false;
  workspace.hidden = true;
  document.querySelector("#page-title").textContent = "Добро пожаловать.";
  document.querySelector("#page-intro").textContent = "Подтвердите вход по приглашению из письма.";
  status.textContent = "";
  return true;
}
let hasInvitation = takeInvitation();
window.addEventListener("hashchange", () => {
  viewRevision += 1;
  hasInvitation = takeInvitation();
});

function csrfToken() {
  const name = location.protocol === "https:" ? "__Host-turnesta_csrf" : "turnesta_csrf";
  const cookie = document.cookie.split(";").map(value => value.trim()).find(value => value.startsWith(`${name}=`));
  return cookie ? cookie.slice(name.length + 1) : "";
}

async function request(url, options = {}) {
  const response = await fetch(url, { credentials: "same-origin", ...options });
  if (!response.ok) {
    if (response.status === 401) throw new Error("Вход недействителен или приглашение уже использовано. Откройте новое приглашение.");
    if (response.status === 403) throw new Error("Запрос не прошёл проверку. Обновите страницу и попробуйте ещё раз.");
    throw new Error("Не удалось выполнить запрос. Попробуйте ещё раз чуть позже.");
  }
  return response.status === 204 ? null : response.json();
}

async function loadOrganizations(append = false, revision = viewRevision) {
  const query = append && nextCursor ? `?after=${encodeURIComponent(nextCursor)}` : "";
  const page = await request(`/api/organizations${query}`);
  if (revision !== viewRevision) return;
  if (!append) list.replaceChildren();
  for (const organization of page.items) {
    const item = document.createElement("li");
    const title = document.createElement("a");
    title.textContent = organization.name;
    title.href = `/app/organizations/${encodeURIComponent(organization.id)}`;
    const role = document.createElement("span");
    role.textContent = ({ owner: "Владелец", manager: "Управляющий", operator: "Оператор", employee: "Сотрудник" })[organization.role] || organization.role;
    item.append(title, role);
    list.append(item);
  }
  nextCursor = page.next_cursor;
  moreButton.hidden = !nextCursor;
  signIn.hidden = true;
  workspace.hidden = false;
  document.querySelector("#page-title").textContent = "Вы в Turnesta.";
  document.querySelector("#page-intro").textContent = "Ваши организации и доступ к рабочему пространству.";
  status.textContent = list.children.length ? "Вы успешно вошли." : "Вы вошли. Администратор ещё не добавил вас в организацию.";
}

form.addEventListener("submit", async (event) => {
  event.preventDefault();
  const revision = ++viewRevision;
  button.disabled = true;
  status.textContent = "Входим…";
  try {
    await request("/auth/login", {
      method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code: input.value.trim() })
    });
    if (revision !== viewRevision) return;
    input.value = "";
    hasInvitation = false;
    status.textContent = "Вы вошли. Открываем кабинет…";
    const password = await fetch("/auth/password", { credentials: "same-origin" })
      .then(response => response.ok ? response.json() : null).catch(() => null);
    if (revision !== viewRevision) return;
    location.replace(password && !password.has_password ? "/login?password=change" : "/app/");
  } catch (error) {
    if (revision === viewRevision) status.textContent = error.message;
  } finally { button.disabled = false; }
});

document.querySelector("#logout-button").addEventListener("click", async (event) => {
  event.currentTarget.disabled = true;
  try {
    await request("/auth/logout", { method: "POST", headers: { "X-CSRF-Token": csrfToken() } });
    location.replace("/login");
  } catch (error) { status.textContent = error.message; event.currentTarget.disabled = false; }
});

moreButton.addEventListener("click", async () => {
  moreButton.disabled = true;
  try { await loadOrganizations(true); } catch (error) { status.textContent = error.message; }
  finally { moreButton.disabled = false; }
});

// An explicit click consumes an invitation. Opening or scanning the link doesn't.
if (!hasInvitation && !changingPassword) {
  const revision = viewRevision;
  fetch("/auth/session", { credentials: "same-origin" }).then(async response => {
    if (revision !== viewRevision || hasInvitation) return;
    if (response.ok) location.replace("/app/");
    else if (response.status !== 401) status.textContent = "Сервис временно недоступен. Попробуйте позже.";
  }).catch(() => { status.textContent = "Нет связи с сервером. Проверьте подключение."; });
}
})();
