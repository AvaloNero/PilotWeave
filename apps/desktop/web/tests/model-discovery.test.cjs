const test = require('node:test');
const assert = require('node:assert/strict');
const { parseHeaders, mergeModels, session } = require('../model-discovery.js');

test('selection preserves manual text and names, deduplicates ignoring case, and enforces the connection limit', () => {
  const original = 'manual | My name\nfixture/chat | Custom name';
  assert.equal(mergeModels(original, [{modelId:'Fixture/Chat',name:'Catalog name'},{modelId:'new-model',name:'New'}]), original+'\nnew-model | New');
  assert.throws(() => mergeModels('manual', Array.from({length:128}, (_,i)=>({modelId:`m${i}`,name:`M${i}`}))), /at most 128/);
  for (const modelId of ['x\ny','x|y','']) assert.throws(()=>mergeModels('',[{modelId,name:'x'}]),/invalid/);
  assert.equal(mergeModels('manual\n', []), 'manual\n');
});

test('headers do not echo credentials in parse errors or mutate object prototypes', () => {
  assert.throws(()=>parseHeaders('PRIVATE_KEY'),error=>!error.message.includes('PRIVATE_KEY'));
  assert.throws(()=>parseHeaders('X-Test: a\nx-test: b'), /Duplicate/);
  const value = parseHeaders('__proto__: safe\nAuthorization: Bearer ${apiKey}');
  assert.equal(Object.getPrototypeOf(value),null); assert.equal(value.Authorization,'Bearer ${apiKey}');
});

test('late results after input edits, modal close, or a replacement form are ignored', async () => {
  let resolve;
  const published = [];
  const job = session(()=>new Promise(r=>{resolve=r;}), r=>published.push(r));
  const first = job.fetch({baseUrl:'old'});
  assert.equal(await job.fetch({baseUrl:'duplicate'}), false);
  job.invalidate(); resolve({models:['old']}); await first;
  assert.deepEqual(published,[]); assert.equal(job.running,false);
  const second = job.fetch({baseUrl:'new'}); resolve({models:['new']}); await second;
  assert.deepEqual(published,[{models:['new']}]);
  const third = job.fetch({baseUrl:'closed'}); job.dispose(); resolve({models:['closed']}); await third;
  assert.equal(published.length,1); assert.equal(await job.fetch({}),false);
});

test('transport errors show a fixed message without echoing input or losing manual models', async () => {
  let result;
  const job = session(async()=>{throw new Error('PRIVATE_KEY');}, r=>{result=r;});
  await job.fetch({apiKey:'PRIVATE_KEY'});
  assert.equal(result.status,'networkError'); assert.deepEqual(result.models,[]);
  assert.doesNotMatch(result.detail,/PRIVATE_KEY/);
});
