import assert from 'node:assert/strict';
import { afterEach, mock, test } from 'node:test';
import { api } from '../src/api.ts';
import { DisplayClient, DisplayError, loadDisplay, validDisplay } from '../../api/web/queue-display-client.mjs';

afterEach(() => { mock.restoreAll(); delete globalThis.document; });
const ticket = n => ({ number: n, service_date: '2026-10-28' });
const snapshot = (count = 0, page = 0, version = '9007199254740993') => ({
  queue_name: 'Queue', location_name: 'Point', organization_name: 'Org', time_zone: 'Europe/Moscow', version,
  observed_at: '2026-10-28T12:00:00.000Z', join_open: false, waiting_count: 7, called_count: count, page,
  next_page: count > (page + 1) * 100 ? page + 1 : null,
  called: Array.from({length: Math.min(100,Math.max(count-page*100,0))},(_,i)=>ticket(page*100+i+1)),
});

test('display loads every page of one version and distinguishes the same number on different days', async () => {
  const pages=[];
  const result=await loadDisplay(async page => { pages.push(page); return snapshot(201,page); });
  assert.deepEqual(pages,[0,1,2]); assert.equal(result.called.length,201); assert.equal(result.version,'9007199254740993');
  const same=snapshot(2); same.called=[ticket(1),{number:1,service_date:'2026-10-27'}];
  assert.equal((await loadDisplay(async()=>same)).called.length,2);
});

test('version changes and duplicate rows never publish a partial display', async () => {
  await assert.rejects(loadDisplay(async page=>snapshot(101,page,page===0?'1':'2')),error=>error.code==='snapshot_changed');
  const duplicate=snapshot(2); duplicate.called=[ticket(1),ticket(1)];
  await assert.rejects(loadDisplay(async()=>duplicate),error=>error.code==='invalid_response');
  assert.equal(validDisplay({...snapshot(),called_count:NaN}),false);
  assert.equal(validDisplay({...snapshot(),next_page:1}),false);
  assert.equal(validDisplay({...snapshot(),version:123}),false);
});

test('failure and explicit unpublication clear previously displayed calls; later GET recovers', async () => {
  let failure=null;
  const client=new DisplayClient('slug',async()=>{if(failure)throw failure;return snapshot(1);});
  await client.refresh(); assert.equal(client.view().called.length,1);
  failure=new DisplayError('unpublished'); await client.refresh(); assert.equal(client.view(),null);
  failure=null; await client.refresh(); assert.equal(client.view().called.length,1);
  failure=new TypeError('network'); await client.refresh(); assert.equal(client.view(),null);
});

test('freshness deadline uses local monotonic time and fails closed at fifteen seconds', async () => {
  let now=100;
  const client=new DisplayClient('slug',async()=>snapshot(1),()=>now);
  await client.refresh(); now=15_099; assert.ok(client.view()); now=15_100; assert.equal(client.view(),null);
  assert.equal(client.error.code,'stale'); await client.refresh(); assert.ok(client.view());
});

test('hidden/offline invalidation rejects late responses without disturbing the new refresh', async () => {
  let finishOld,finishNew; let count=0;
  const client=new DisplayClient('slug',()=>new Promise(resolve=>{if(count++===0)finishOld=resolve;else finishNew=resolve;}));
  const old=client.refresh(); client.invalidate('stale'); const current=client.refresh();
  finishOld(snapshot(1)); await old; assert.equal(client.view(),null); assert.equal(client.busy,true);
  finishNew(snapshot(2)); await current; assert.equal(client.view().called.length,2); assert.equal(client.busy,false);
  client.invalidate('offline'); assert.equal(client.view(),null);
});

test('one refresh at a time; aborting a multi-page request cannot publish it', async () => {
  let finish; let count=0;
  const client=new DisplayClient('slug',()=>{count++;return new Promise(resolve=>{finish=resolve;});});
  const first=client.refresh(); await client.refresh(); assert.equal(count,1);
  client.invalidate('offline'); finish(snapshot(1)); await first; assert.equal(client.view(),null);
  const controller=new AbortController();
  await assert.rejects(loadDisplay(async()=>{controller.abort();return snapshot(101);},controller.signal));
});

test('anonymous display reads omit credentials, carry only page number and map 404 without login', async () => {
  mock.method(globalThis,'fetch',async(url,options)=>{
    assert.equal(url,'/api/public/queues/slug/display?page=0'); assert.equal(options.credentials,'omit'); assert.equal(options.cache,'no-store');
    assert.equal(options.method,undefined); assert.equal(options.headers,undefined); return new Response('',{status:404});
  });
  const client=new DisplayClient('slug'); await client.refresh(); assert.equal(client.error.code,'unpublished');
});

test('display configuration sends its own versioned PUT with CSRF and never changes joining', async () => {
  globalThis.document={cookie:'turnesta_csrf=csrf'};
  mock.method(globalThis,'fetch',async(url,options)=>{
    assert.equal(url,'/api/organizations/tenant/queues/queue/public-access/display'); assert.equal(options.method,'PUT');
    assert.deepEqual(JSON.parse(options.body),{enabled:true,expected_version:3}); assert.equal(options.headers.get('X-CSRF-Token'),'csrf');
    return Response.json({display_enabled:true,display_url:'/display/slug',join_open:false,version:4});
  });
  assert.equal((await api.configureQueueDisplay('tenant','queue',true,3)).join_open,false);
});


test('display keeps optional station labels across pages and rejects malformed labels', async () => {
  const result=await loadDisplay(async page=>{const data=snapshot(101,page);data.called=data.called.map(t=>({...t,station_name:'Window <1>'}));return data;});
  assert.equal(result.called[100].station_name,'Window <1>');
  for(const invalid of ['',42,{}]) {const data=snapshot(1);data.called[0].station_name=invalid;assert.equal(validDisplay(data),false);}
  const legacy=snapshot(1);assert.equal(validDisplay(legacy),true);
});
