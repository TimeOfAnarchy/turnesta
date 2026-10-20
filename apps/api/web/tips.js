(() => {
  'use strict';
  const create = document.querySelector('[data-create-attempt]');
  const complete = document.querySelector('[data-complete-checkout]');
  const status = document.querySelector('[data-payment-status]');

  function errorText(error) {
    if (error.code === 'invalid_amount') return 'Введите сумму от 1 до 10 000 ₽, не больше двух знаков после запятой.';
    if (error.status === 409) return 'Операция уже записана или данные запроса изменились. Обновите страницу, чтобы увидеть текущий результат.';
    if (error.status === 404) return 'Ссылка больше недоступна. Запросите актуальную ссылку у получателя.';
    return 'Не удалось получить подтверждение. Проверьте соединение и попробуйте ещё раз.';
  }
  async function request(url, body) {
    const response = await fetch(url, body === undefined ? {credentials:'omit',cache:'no-store'} : {method:'POST',headers:{'Content-Type':'application/json'},credentials:'omit',body:JSON.stringify(body)});
    const data = await response.json().catch(() => ({}));
    if (!response.ok) throw {status:response.status,code:data.code};
    return data;
  }
  function busy(form, value) { for (const element of form.querySelectorAll('button,input')) element.disabled = value; }
  function showError(element, error) { element.textContent = errorText(error); element.hidden = false; }
  function redirect(path, prefix) {
    if (typeof path !== 'string' || !path.startsWith(prefix) || !/^\/[a-z/-]+[0-9a-f-]{36}$/.test(path)) throw {};
    window.location.assign(path);
  }
  function minorUnits(value) {
    const parts = /^(0|[1-9]\d{0,4})(?:[.,](\d{1,2}))?$/.exec(value.trim());
    if (!parts) return null;
    const minor = Number(parts[1]) * 100 + Number((parts[2] || '').padEnd(2, '0'));
    return Number.isSafeInteger(minor) && minor >= 100 && minor <= 1000000 ? minor : null;
  }
  if (create) {
    const amount = create.elements.namedItem('amount');
    const error = document.getElementById('form-error');
    const presets = create.querySelectorAll('[data-amount]');
    function reset() {
      create.dataset.idempotencyKey = crypto.randomUUID();
      for (const button of presets) button.setAttribute('aria-pressed',String(button.dataset.amount === amount.value));
      error.hidden = true;
    }
    for (const button of presets) button.addEventListener('click',() => { amount.value = button.dataset.amount; reset(); });
    amount.addEventListener('input',reset);
    create.addEventListener('submit',async event => {
      event.preventDefault();
      const minor = minorUnits(amount.value);
      if (minor === null) { showError(error,{code:'invalid_amount'}); amount.focus(); return; }
      busy(create,true); error.hidden = true;
      try {
        const result = await request(`/api/public/tips/${create.dataset.slug}/attempts`,{amount_minor:minor,idempotency_key:create.dataset.idempotencyKey});
        redirect(result.checkout_url,'/mock-payments/');
      } catch (failure) { showError(error,failure); busy(create,false); }
    });
  }
  if (complete) {
    const error = document.getElementById('form-error');
    complete.addEventListener('submit',async event => {
      event.preventDefault();
      const outcome = event.submitter?.value;
      if (!['succeeded','failed'].includes(outcome)) return;
      busy(complete,true); error.hidden = true;
      try {
        const result = await request(`/api/mock-payments/${complete.dataset.token}/complete`,{outcome});
        redirect(result.status_url,'/tips/status/');
      } catch (failure) { showError(error,failure); busy(complete,false); }
    });
  }
  if (status) {
    const messages = {
      succeeded:['Спасибо за добрые слова и чаевые!','Тестовая оплата подтверждена сервером. Деньги не списывались.'],
      failed:['Тестовая оплата отклонена','Эмулятор вернул отказ. Деньги не списывались.'],
      expired:['Время тестовой оплаты истекло','Откройте ссылку получателя, чтобы начать заново. Деньги не списывались.'],
      awaiting_payment:['Проверяем тестовую оплату','Ждём серверное подтверждение от эмулятора. Возврат на эту страницу сам по себе не подтверждает оплату.'],
    };
    const refresh = document.getElementById('refresh-status');
    const error = document.getElementById('status-error');
    let polls = 0;
    let timer;
    async function update() {
      window.clearTimeout(timer); refresh.disabled = true;
      try {
        const result = await request(`/api/public/tips/status/${status.dataset.token}`);
        const text = messages[result.status];
        if (!text) throw {};
        status.dataset.status = result.status;
        document.getElementById('status-heading').textContent = text[0];
        document.getElementById('status-message').textContent = text[1];
        error.hidden = true;
        if (result.status === 'awaiting_payment' && ++polls < 30) timer = window.setTimeout(update,2000);
        else if (result.status === 'awaiting_payment') document.getElementById('status-message').textContent = 'Подтверждение пока не получено. Можно проверить статус позже; повторно оплачивать эту операцию не нужно.';
      } catch (failure) { showError(error,failure); }
      finally { refresh.disabled = false; }
    }
    refresh.addEventListener('click',() => { polls = 0; void update(); });
    if (status.dataset.status === 'awaiting_payment') timer = window.setTimeout(update,1500);
    window.addEventListener('pagehide',() => window.clearTimeout(timer),{once:true});
  }
})();
