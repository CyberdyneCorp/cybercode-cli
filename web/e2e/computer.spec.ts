import { test, expect, login, select, tab } from './browser.js';

// HTTP routes simulate native state; this suite never requests OS consent,
// captures a desktop or injects host input.
test('native global authority, exact review, private preview and logout Stop are wired into the browser', async ({page,dashboard}) => {
  const first=await dashboard.session('Native owner');await dashboard.session('Other Session');
  const target={identity:'app:test.fixture:42:100:window:7',app:'test.fixture',display:1,pid:42,window:7,x:0,y:0,width:100,height:100,image_width:100,image_height:100};
  let grant:unknown={id:'cug_other',session_id:'ses_other',target:{identity:target.identity},expires_at:Date.now()+300000};
  const writes:{path:string;body:unknown}[]=[];let frameReads=0;
  await page.route('**/api/v1/tools/computer/**',async route=>{
    const path=new URL(route.request().url()).pathname;
    if(path.endsWith('/health'))return route.fulfill({json:{data:{enabled:true,health:{supported:true,platform:'fixture',minimum_version:'fixture',screen_capture:true,accessibility:true,binary:'fixture'},grant}}});
    writes.push({path,body:route.request().postDataJSON()});if(path.endsWith('/stop'))grant=null;
    return route.fulfill({json:{data:{}}});
  });
  await page.route('**/api/v1/sessions/*/computer**',async route=>{
    const request=route.request();const path=new URL(request.url()).pathname;
    if(path.endsWith('/preview')){writes.push({path,body:request.postDataJSON()});return route.fulfill({json:{data:{id:'cup_review',session_id:first,target,expires_at:Date.now()+30000,unknown_operations:[],warning:'Review exact fixture target'}}});}
    if(path.endsWith('/allow')){writes.push({path,body:request.postDataJSON()});grant={id:'cug_new',session_id:first,target,expires_at:Date.now()+300000};return route.fulfill({json:{data:grant}});}
    return route.fulfill({json:{data:{latest_frame:{id:'cuf_fixture',session_id:first,created_ms:Date.now(),bytes:68,sha256:'fixture',target}}}});
  });
  await page.route('**/api/v1/sessions/*/computer/frames/*',async route=>{frameReads++;expect(route.request().headers()['authorization']).toMatch(/^Basic /);return route.fulfill({contentType:'image/png',body:Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+j7foAAAAASUVORK5CYII=','base64')});});
  await login(page,dashboard);await select(page,'Native owner');
  const controls=page.getByRole('region',{name:'Native computer controls'});
  await expect(controls.getByRole('alert')).toContainText('ses_other');
  await controls.getByRole('button',{name:'STOP native input',exact:true}).click();
  await expect(controls.getByRole('alert')).toHaveCount(0);
  await controls.locator('summary').click();
  await controls.getByLabel('Application bundle ID or display ID').fill('test.fixture');
  await controls.getByRole('button',{name:'Review native target',exact:true}).click();
  const arm=controls.getByRole('button',{name:'Arm reviewed target',exact:true});await expect(arm).toBeDisabled();
  await controls.getByRole('checkbox',{name:'I reviewed this exact target and authorize native capture/input'}).check();await arm.click();
  await expect(controls.getByRole('alert')).toContainText('ARMED');expect(frameReads).toBe(0);
  await dashboard.prompt(first,'NATIVE_VIEWPORT_ROWS\n'+Array.from({length:80},(_,index)=>`NATIVE_VIEWPORT_ROW_${index}`).join('\n'));
  await expect(page.getByText('NATIVE_VIEWPORT_ROW_79',{exact:false})).toBeVisible();
  await page.evaluate(()=>window.scrollTo(0,document.body.scrollHeight));
  await expect(controls.getByRole('button',{name:'STOP native input',exact:true})).toBeInViewport();
  await page.evaluate(()=>window.scrollTo(0,0));

  await controls.getByRole('button',{name:'View authorized capture',exact:true}).click();
  await expect(controls.getByAltText('Authorized private native screenshot')).toHaveJSProperty('naturalWidth',1);expect(frameReads).toBe(1);
  await select(page,'Other Session');await expect(controls.getByRole('alert')).toContainText(first);
  await select(page,'Native owner');await dashboard.prompt(first,'WRITE_BROWSER:native_modal');await dashboard.waitPending('permissions',first);await tab(page,'Approvals');
  await page.getByRole('button',{name:'Review saved approval',exact:true}).click();
  const dialog=page.getByRole('dialog',{name:'Review saved approval'});await dialog.getByRole('button',{name:'STOP native input',exact:true}).click();
  await expect(dialog.getByRole('button',{name:'STOP native input',exact:true})).toHaveCount(0);
  await dialog.getByRole('button',{name:'Cancel',exact:true}).click();

  await page.getByRole('button',{name:'Logout',exact:true}).click();await expect(page.getByLabel('Password',{exact:true})).toBeVisible();
  await expect.poll(()=>writes.filter(write=>write.path.endsWith('/stop')).length).toBe(3);
  expect(writes.find(write=>write.path.endsWith('/allow'))?.body).toEqual({preview_id:'cup_review',confirm:true,ttl_seconds:300,acknowledge_unknown:false});
});
