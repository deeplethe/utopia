import test from 'node:test';
import assert from 'node:assert/strict';
import {build,conclude,reopen,preview,patch,declaration} from './policy.mjs';
const a=[{id:'r',key:'revenue',label:'Amount',datatype:'number',unit:'USD'},{id:'c',key:'cost',label:'Amount',datatype:'number',unit:'USD'},{id:'e',key:'euro',label:'Amount',datatype:'number',unit:'EUR'},{id:'ratio',key:'ratio',label:'Ratio',datatype:'number',unit:'1'}];
const attr=id=>({kind:'attr',id}), num=raw=>({kind:'const',raw}), op=(op,l,r)=>({kind:'arith',op,l,r});
const margin=op('sub',attr('r'),attr('c'));
test('same-unit subtraction and dimensionless scaling preserve the tree',()=>{
 const tree=op('mul',margin,num('1.1')); const ast=conclude(tree,'r',a);
 assert.deepEqual(ast,{op:'mul',l:{op:'sub',l:{attr:'r'},r:{attr:'c'}},r:{const:1.1}});
 assert.deepEqual(conclude(reopen(ast),'r',a),ast);
});
test('unknown, missing, offset, ambiguous and compound units are not unitless',()=>{
 for(const unit of [null,undefined,'','1','$','¥','%','bp','°C','USD/year']) assert.throws(()=>declaration({...a[0],unit}));
 assert.equal(declaration(a[3],true),'1');
});
test('mismatched add/sub, dimensioned multiplication and bare subtract are rejected',()=>{
 for(const tree of [op('add',attr('r'),attr('e')),op('sub',attr('r'),num('1')),op('mul',attr('r'),attr('c')),op('div',attr('r'),attr('e'))]) assert.throws(()=>build(tree,a),/incompatible_units/);
});
test('ratio requires separately approved explicit-unitless target',()=>{
 const tree=op('div',margin,attr('r'));
 assert.throws(()=>conclude(tree,'ratio',a),/unknown_unit/);
 assert.deepEqual(conclude(tree,'ratio',a,{allowUnitless:true}),{op:'div',l:{op:'sub',l:{attr:'r'},r:{attr:'c'}},r:{attr:'r'}});
 assert.throws(()=>conclude(tree,'r',a,{allowUnitless:true}),/target_unit_mismatch/);
});
test('draft fragments, non-finite and non-decimal strings never become zero',()=>{
 for(const raw of ['', ' ','-','1e','NaN','Infinity','0x10','1e999']) assert.throws(()=>build(num(raw),a));
 for(const raw of ['0','-2','.5','1e2']) assert.equal(build(num(raw),a).ast.const,Number(raw));
});
test('missing and nonnumeric attributes reject; labels never identify nodes',()=>{
 assert.throws(()=>build(attr('missing'),a));
 assert.throws(()=>build(attr('r'),[{...a[0],datatype:'text'}]));
 assert.match(preview(build(margin,a).ast,a),/revenue.*cost/);
 const renamed=a.map(x=>({...x,label:'Renamed'}));
 assert.deepEqual(build(margin,a).ast,build(margin,renamed).ast);
});
test('depth boundary is four edges and five nodes, next edge rejects',()=>{
 let tree=attr('r'); for(let i=0;i<4;i++)tree=op('mul',tree,num('1'));
 assert.ok(build(tree,a)); assert.throws(()=>build(op('mul',tree,num('1')),a),/expression_too_deep/);
});
test('unknown and mixed legacy shapes remain protected',()=>{
 for(const ast of [{attr:'r',extra:true},{const:'2'},{op:'pow',l:{attr:'r'},r:{const:2}},{attr:'r',const:2}]) assert.equal(reopen(ast),null);
});
test('metadata-only patch does not rewrite a legacy definition or condition groups',()=>{
 const original={conclude_expr:{future:'unknown'},conditions:[{group:2},{group:7}]};
 assert.deepEqual(patch(original,{name:'New',description:'D'}),{name:'New',description:'D'});
 assert.deepEqual(patch(original,{name:'New',description:'D'},original),{name:'New',description:'D'});
 const changed={...original,conclude_expr:{attr:'r'}};
 assert.deepEqual(patch(original,{name:'New',description:'D'},changed).conditions,[{group:2},{group:7}]);
});
test('constant-only computed rules stay rejected',()=>assert.throws(()=>conclude(num('4'),'r',a),/constant_expression/));
